//! Android glue over JNI: the intent diari was started with (share or
//! quick-add shortcut), the long-press "Quick add" shortcut, opening links
//! and attached files in other apps, and finishing the activity.

use jni::objects::{JByteArray, JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};
use slint::android::AndroidApp;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::{attachments, Launch, SharedFile};

/// Set while diari itself opens another app (a link or a file), so leaving
/// for it doesn't finish the activity and the user comes back to the note.
static OPENING_OTHER_APP: AtomicBool = AtomicBool::new(false);

/// For the Stop handler: true if this Stop is diari opening another app.
pub fn take_opening_other_app() -> bool {
    OPENING_OTHER_APP.swap(false, Ordering::SeqCst)
}

/// Where opened attachments are copied so other apps can read them.
const EXPORT_DIR: &str = "Download/diari/";
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x0000_0001;
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
const COPY_CHUNK: usize = 64 * 1024;

/// The running activity, so app code can finish it (see `finish_current`).
static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);

pub fn set_app(app: &AndroidApp) {
    *APP.lock().unwrap() = Some(app.clone());
}

/// Finish the running activity, returning to whatever was shown before.
pub fn finish_current() {
    if let Some(app) = APP.lock().unwrap().clone() {
        finish(&app);
    }
}

/// Action of the "Quick add" launcher shortcut.
/// Tied to the Android package ID, which stays `dev.perb.app` (the app was
/// first called perb) so that updates keep the installed app's data.
const QUICK_ADD_ACTION: &str = "dev.perb.app.QUICK_ADD";

/// Run `f` with a JNI env and the activity object. Logs and returns `None`
/// on any JNI error (including a pending Java exception).
fn with_activity<R>(
    app: &AndroidApp,
    f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    // A global reference owned by android-activity; JObject never deletes it.
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let res = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    res.map_err(|e| log::warn!("jni: {e}")).ok()
}

fn to_string(env: &mut JNIEnv, o: JObject) -> jni::errors::Result<Option<String>> {
    if o.is_null() {
        return Ok(None);
    }
    Ok(Some(env.get_string(&JString::from(o))?.into()))
}

fn string_extra(env: &mut JNIEnv, intent: &JObject, key: &str) -> jni::errors::Result<Option<String>> {
    let key = env.new_string(key)?;
    let v = env
        .call_method(intent, "getStringExtra", "(Ljava/lang/String;)Ljava/lang/String;", &[JValue::Object(&key)])?
        .l()?;
    to_string(env, v)
}

/// Why diari was started, if not from the launcher: text and/or files shared
/// to it (files are copied into `staging`), or the "Quick add" shortcut.
pub fn launch_request(app: &AndroidApp, staging: &Path) -> Option<Launch> {
    with_activity(app, |env, activity| {
        let intent = env.call_method(activity, "getIntent", "()Landroid/content/Intent;", &[])?.l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let action = env.call_method(&intent, "getAction", "()Ljava/lang/String;", &[])?.l()?;
        match to_string(env, action)?.as_deref() {
            Some(action @ ("android.intent.action.SEND" | "android.intent.action.SEND_MULTIPLE")) => {
                let text = string_extra(env, &intent, "android.intent.extra.TEXT")?.unwrap_or_default();
                let subject = string_extra(env, &intent, "android.intent.extra.SUBJECT")?.unwrap_or_default();
                let uris = shared_uris(env, &intent, action == "android.intent.action.SEND_MULTIPLE")?;
                let mut files = Vec::new();
                for (i, uri) in uris.iter().enumerate() {
                    match copy_shared(env, activity, uri, staging, i) {
                        Ok(f) => files.push(f),
                        Err(e) => {
                            // Don't let one unreadable file fail the others.
                            if env.exception_check().unwrap_or(false) {
                                let _ = env.exception_describe();
                                let _ = env.exception_clear();
                            }
                            log::warn!("could not read shared file {i}: {e}");
                        }
                    }
                }
                Ok(Some(Launch::Share { text: combine_share(&subject, &text), files }))
            }
            Some(QUICK_ADD_ACTION) => Ok(Some(Launch::QuickAdd)),
            _ => Ok(None),
        }
    })
    .flatten()
}

/// The `content://` URIs of shared files (EXTRA_STREAM).
fn shared_uris<'a>(env: &mut JNIEnv<'a>, intent: &JObject, multiple: bool) -> jni::errors::Result<Vec<JObject<'a>>> {
    let key = env.new_string("android.intent.extra.STREAM")?;
    if !multiple {
        let uri = env
            .call_method(intent, "getParcelableExtra", "(Ljava/lang/String;)Landroid/os/Parcelable;", &[JValue::Object(&key)])?
            .l()?;
        return Ok(if uri.is_null() { vec![] } else { vec![uri] });
    }
    let list = env
        .call_method(intent, "getParcelableArrayListExtra", "(Ljava/lang/String;)Ljava/util/ArrayList;", &[JValue::Object(&key)])?
        .l()?;
    if list.is_null() {
        return Ok(vec![]);
    }
    let n = env.call_method(&list, "size", "()I", &[])?.i()?;
    let mut out = Vec::new();
    for i in 0..n {
        let uri = env.call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?.l()?;
        if !uri.is_null() {
            out.push(uri);
        }
    }
    Ok(out)
}

fn content_resolver<'a>(env: &mut JNIEnv<'a>, activity: &JObject) -> jni::errors::Result<JObject<'a>> {
    env.call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()
}

/// Copy one shared file into `staging`, keeping the name the sending app gave it.
fn copy_shared(env: &mut JNIEnv, activity: &JObject, uri: &JObject, staging: &Path, i: usize) -> Result<SharedFile, String> {
    let e = |e: jni::errors::Error| e.to_string();
    let resolver = content_resolver(env, activity).map_err(e)?;
    let mime = env
        .call_method(&resolver, "getType", "(Landroid/net/Uri;)Ljava/lang/String;", &[JValue::Object(uri)])
        .and_then(|v| v.l())
        .map_err(e)?;
    let mime = to_string(env, mime).map_err(e)?.unwrap_or_default();
    let name = display_name_or_none(env, &resolver, uri)
        .unwrap_or_else(|| format!("shared-{}.{}", i + 1, attachments::ext_for_mime(&mime)));

    let stream = env
        .call_method(&resolver, "openInputStream", "(Landroid/net/Uri;)Ljava/io/InputStream;", &[JValue::Object(uri)])
        .and_then(|v| v.l())
        .map_err(e)?;
    if stream.is_null() {
        return Err("no data".into());
    }
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
    let path: PathBuf = staging.join(format!("{i}-{}", attachments::sanitize(&name)));
    let mut out = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let buf = env.new_byte_array(COPY_CHUNK as i32).map_err(e)?;
    let mut chunk = vec![0i8; COPY_CHUNK];
    let res = loop {
        let n = match env.call_method(&stream, "read", "([B)I", &[JValue::Object(&buf)]).and_then(|v| v.i()) {
            Ok(n) => n,
            Err(err) => break Err(err.to_string()),
        };
        if n < 0 {
            break Ok(());
        }
        let n = n as usize;
        if let Err(err) = env.get_byte_array_region(&buf, 0, &mut chunk[..n]) {
            break Err(err.to_string());
        }
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(chunk.as_ptr() as *const u8, n) };
        if let Err(err) = out.write_all(bytes) {
            break Err(err.to_string());
        }
    };
    let _ = env.call_method(&stream, "close", "()V", &[]);
    res.map(|_| SharedFile { path, name })
}

/// OpenableColumns.DISPLAY_NAME of a content URI.
fn display_name(env: &mut JNIEnv, resolver: &JObject, uri: &JObject) -> Option<String> {
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[JValue::Object(uri), JValue::Object(&JObject::null()), JValue::Object(&JObject::null()), JValue::Object(&JObject::null()), JValue::Object(&JObject::null())],
        )
        .ok()?
        .l()
        .ok()?;
    if cursor.is_null() {
        return None;
    }
    let mut name = None;
    if env.call_method(&cursor, "moveToFirst", "()Z", &[]).ok()?.z().ok()? {
        let col = env.new_string("_display_name").ok()?;
        let idx = env.call_method(&cursor, "getColumnIndex", "(Ljava/lang/String;)I", &[JValue::Object(&col)]).ok()?.i().ok()?;
        if idx >= 0 {
            let v = env.call_method(&cursor, "getString", "(I)Ljava/lang/String;", &[JValue::Int(idx)]).ok()?.l().ok()?;
            name = to_string(env, v).ok()?;
        }
    }
    let _ = env.call_method(&cursor, "close", "()V", &[]);
    name.filter(|n| !n.trim().is_empty())
}

/// `display_name` without leaving a Java exception pending (some providers
/// don't support queries; the copy can still go ahead).
fn display_name_or_none(env: &mut JNIEnv, resolver: &JObject, uri: &JObject) -> Option<String> {
    let name = display_name(env, resolver, uri);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    name
}

fn start_activity(env: &mut JNIEnv, activity: &JObject, intent: &JObject) -> jni::errors::Result<()> {
    OPENING_OTHER_APP.store(true, Ordering::SeqCst);
    let res = env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(intent)]);
    if res.is_err() {
        OPENING_OTHER_APP.store(false, Ordering::SeqCst);
    }
    res.map(|_| ())
}

fn current_app() -> Result<AndroidApp, String> {
    APP.lock().unwrap().clone().ok_or_else(|| "app not ready".to_string())
}

/// Open a web page (or mail link, …) in the app that handles it.
pub fn open_url(url: &str) -> Result<(), String> {
    let app = current_app()?;
    with_activity(&app, |env, activity| {
        let s = env.new_string(url)?;
        let uri = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&s)])?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)])?;
        start_activity(env, activity, &intent)
    })
    .ok_or_else(|| "no app can open this link".to_string())
}

/// Open an attached file in the app that handles its type. Other apps can't
/// read diari's private files, so it's copied to Downloads/diari first (named
/// after its note, and reused if it's already there).
pub fn open_file(path: &Path, note: &str) -> Result<(), String> {
    let app = current_app()?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let note_stem = note.rsplit('/').next().unwrap_or(note).trim_end_matches(".md");
    let export_name = format!("{note_stem}-{file_name}");
    let mime = attachments::mime_for(&file_name);
    let mut data = Vec::new();
    std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut data)).map_err(|e| e.to_string())?;

    with_activity(&app, |env, activity| {
        let resolver = content_resolver(env, activity)?;
        // MediaStore.Downloads exists from Android 10 (API 29).
        let collection = env
            .get_static_field("android/provider/MediaStore$Downloads", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?
            .l()?;
        let uri = match find_export(env, &resolver, &collection, &export_name)? {
            Some(uri) => uri,
            None => {
                let values = env.new_object("android/content/ContentValues", "()V", &[])?;
                for (k, v) in [("_display_name", export_name.as_str()), ("mime_type", mime), ("relative_path", EXPORT_DIR)] {
                    let (k, v) = (env.new_string(k)?, env.new_string(v)?);
                    env.call_method(&values, "put", "(Ljava/lang/String;Ljava/lang/String;)V", &[JValue::Object(&k), JValue::Object(&v)])?;
                }
                let uri = env
                    .call_method(
                        &resolver,
                        "insert",
                        "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
                        &[JValue::Object(&collection), JValue::Object(&values)],
                    )?
                    .l()?;
                let out = env
                    .call_method(&resolver, "openOutputStream", "(Landroid/net/Uri;)Ljava/io/OutputStream;", &[JValue::Object(&uri)])?
                    .l()?;
                for chunk in data.chunks(COPY_CHUNK) {
                    let arr: JByteArray = env.byte_array_from_slice(chunk)?;
                    env.call_method(&out, "write", "([B)V", &[JValue::Object(&arr)])?;
                    env.delete_local_ref(arr)?;
                }
                env.call_method(&out, "close", "()V", &[])?;
                uri
            }
        };
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        let mime_s = env.new_string(mime)?;
        env.call_method(
            &intent,
            "setDataAndType",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&uri), JValue::Object(&mime_s)],
        )?;
        env.call_method(
            &intent,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(FLAG_GRANT_READ_URI_PERMISSION | FLAG_ACTIVITY_NEW_TASK)],
        )?;
        start_activity(env, activity, &intent)
    })
    .ok_or_else(|| "no app can open this file here (opening files needs Android 10 or later)".to_string())
}

/// A copy of `name` already exported to Downloads/diari by diari.
fn find_export<'a>(env: &mut JNIEnv<'a>, resolver: &JObject, collection: &JObject, name: &str) -> jni::errors::Result<Option<JObject<'a>>> {
    let id_col = env.new_string("_id")?;
    let projection: JObjectArray = env.new_object_array(1, "java/lang/String", &id_col)?;
    let selection = env.new_string("_display_name=? AND relative_path=?")?;
    let (n, d) = (env.new_string(name)?, env.new_string(EXPORT_DIR)?);
    let args: JObjectArray = env.new_object_array(2, "java/lang/String", &n)?;
    env.set_object_array_element(&args, 1, &d)?;
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[JValue::Object(collection), JValue::Object(&projection), JValue::Object(&selection), JValue::Object(&args), JValue::Object(&JObject::null())],
        )?
        .l()?;
    if cursor.is_null() {
        return Ok(None);
    }
    let mut found = None;
    if env.call_method(&cursor, "moveToFirst", "()Z", &[])?.z()? {
        let id = env.call_method(&cursor, "getLong", "(I)J", &[JValue::Int(0)])?.j()?;
        found = Some(
            env.call_static_method(
                "android/content/ContentUris",
                "withAppendedId",
                "(Landroid/net/Uri;J)Landroid/net/Uri;",
                &[JValue::Object(collection), JValue::Long(id)],
            )?
            .l()?,
        );
    }
    env.call_method(&cursor, "close", "()V", &[])?;
    Ok(found)
}

/// Shared links often come as subject = page title, text = URL.
fn combine_share(subject: &str, text: &str) -> String {
    let (subject, text) = (subject.trim(), text.trim());
    if subject.is_empty() || text.contains(subject) {
        text.to_string()
    } else if text.is_empty() {
        subject.to_string()
    } else {
        format!("{subject} {text}")
    }
}

/// Register the "Quick add" shortcut shown when long-pressing the app icon.
pub fn register_quick_add_shortcut(app: &AndroidApp) {
    with_activity(app, |env, activity| {
        let action = env.new_string(QUICK_ADD_ACTION)?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        let class_name = env.new_string("android.app.NativeActivity")?;
        env.call_method(
            &intent,
            "setClassName",
            "(Landroid/content/Context;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(activity), JValue::Object(&class_name)],
        )?;

        let id = env.new_string("quick-add")?;
        let builder = env.new_object(
            "android/content/pm/ShortcutInfo$Builder",
            "(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue::Object(activity), JValue::Object(&id)],
        )?;
        let short = env.new_string("Quick add")?;
        let long = env.new_string("Add a line to today")?;
        let set = "(Ljava/lang/CharSequence;)Landroid/content/pm/ShortcutInfo$Builder;";
        env.call_method(&builder, "setShortLabel", set, &[JValue::Object(&short)])?;
        env.call_method(&builder, "setLongLabel", set, &[JValue::Object(&long)])?;
        env.call_method(
            &builder,
            "setIntent",
            "(Landroid/content/Intent;)Landroid/content/pm/ShortcutInfo$Builder;",
            &[JValue::Object(&intent)],
        )?;
        // The app icon ("D") as the shortcut's icon.
        let res = env.call_method(activity, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
        let (name, kind, pkg) = (env.new_string("ic_launcher")?, env.new_string("mipmap")?, env.new_string("dev.perb.app")?);
        let icon_id = env
            .call_method(
                &res,
                "getIdentifier",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                &[JValue::Object(&name), JValue::Object(&kind), JValue::Object(&pkg)],
            )?
            .i()?;
        if icon_id != 0 {
            let icon = env
                .call_static_method(
                    "android/graphics/drawable/Icon",
                    "createWithResource",
                    "(Landroid/content/Context;I)Landroid/graphics/drawable/Icon;",
                    &[JValue::Object(activity), JValue::Int(icon_id)],
                )?
                .l()?;
            env.call_method(
                &builder,
                "setIcon",
                "(Landroid/graphics/drawable/Icon;)Landroid/content/pm/ShortcutInfo$Builder;",
                &[JValue::Object(&icon)],
            )?;
        }
        let info = env.call_method(&builder, "build", "()Landroid/content/pm/ShortcutInfo;", &[])?.l()?;

        let list = env
            .call_static_method(
                "java/util/Collections",
                "singletonList",
                "(Ljava/lang/Object;)Ljava/util/List;",
                &[JValue::Object(&info)],
            )?
            .l()?;
        let sm_class = env.find_class("android/content/pm/ShortcutManager")?;
        let sm = env
            .call_method(
                activity,
                "getSystemService",
                "(Ljava/lang/Class;)Ljava/lang/Object;",
                &[JValue::Object(&sm_class)],
            )?
            .l()?;
        if !sm.is_null() {
            env.call_method(&sm, "setDynamicShortcuts", "(Ljava/util/List;)Z", &[JValue::Object(&list)])?;
        }
        Ok(())
    });
}

/// Finish the activity, so the next launch (from the launcher, a share or the
/// shortcut) starts fresh with its own intent. NativeActivity ignores intents
/// delivered to an already running instance.
pub fn finish(app: &AndroidApp) {
    with_activity(app, |env, activity| env.call_method(activity, "finish", "()V", &[]).map(|_| ()));
}

fn sdk_int(env: &mut JNIEnv) -> i32 {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I").and_then(|v| v.i()).unwrap_or(0)
}

/// Open the camera app to take a picture into a new gallery item named
/// `name` (in Pictures/diari). Returns that item's URI, to import the
/// picture when diari comes back (`import_photo`).
pub fn take_photo(name: &str) -> Result<String, String> {
    const FLAG_GRANT_READ_WRITE: i32 = 0x1 | 0x2;
    let app = current_app()?;
    let res = with_activity(&app, |env, activity| {
        let resolver = content_resolver(env, activity)?;
        let collection = env
            .get_static_field("android/provider/MediaStore$Images$Media", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?
            .l()?;
        let values = env.new_object("android/content/ContentValues", "()V", &[])?;
        let mut put = |env: &mut JNIEnv, k: &str, v: &str| -> jni::errors::Result<()> {
            let (k, v) = (env.new_string(k)?, env.new_string(v)?);
            env.call_method(&values, "put", "(Ljava/lang/String;Ljava/lang/String;)V", &[
                JValue::Object(&k),
                JValue::Object(&v),
            ])?;
            Ok(())
        };
        put(env, "_display_name", name)?;
        put(env, "mime_type", "image/jpeg")?;
        if sdk_int(env) >= 29 {
            put(env, "relative_path", "Pictures/diari")?;
        }
        let uri = env
            .call_method(&resolver, "insert", "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;", &[
                JValue::Object(&collection),
                JValue::Object(&values),
            ])?
            .l()?;
        if uri.is_null() {
            return Ok(Err("could not make room for the picture".to_string()));
        }
        let action = env.new_string("android.media.action.IMAGE_CAPTURE")?;
        let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&action)])?;
        let key = env.new_string("output")?;
        env.call_method(&intent, "putExtra", "(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;", &[
            JValue::Object(&key),
            JValue::Object(&uri),
        ])?;
        env.call_method(&intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(FLAG_GRANT_READ_WRITE)])?;
        if start_activity(env, activity, &intent).is_err() {
            // No camera app: drop the empty gallery item again.
            let _ = env.exception_clear();
            let _ = env.call_method(&resolver, "delete", "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I", &[
                JValue::Object(&uri),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ]);
            return Ok(Err("no camera app found".to_string()));
        }
        let s = env.call_method(&uri, "toString", "()Ljava/lang/String;", &[])?.l()?;
        Ok(to_string(env, s)?.ok_or_else(|| "no picture address".to_string()))
    });
    res.unwrap_or_else(|| Err("could not open the camera".into()))
}

/// After the camera: copy the picture at `uri` into `staging` and remove it
/// from the gallery (it lives in the note now). `None` if no picture was
/// taken.
pub fn import_photo(uri: &str, staging: &Path) -> Result<Option<SharedFile>, String> {
    let app = current_app()?;
    let res = with_activity(&app, |env, activity| {
        let s = env.new_string(uri)?;
        let u = env
            .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&s)])?
            .l()?;
        let copied = copy_shared(env, activity, &u, staging, 0);
        if env.exception_check()? {
            env.exception_clear()?;
        }
        let resolver = content_resolver(env, activity)?;
        let _ = env.call_method(&resolver, "delete", "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I", &[
            JValue::Object(&u),
            JValue::Object(&JObject::null()),
            JValue::Object(&JObject::null()),
        ]);
        Ok(copied)
    });
    match res {
        Some(Ok(f)) => {
            if std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0) == 0 {
                let _ = std::fs::remove_file(&f.path);
                Ok(None)
            } else {
                Ok(Some(f))
            }
        }
        // Cancelled: the camera never wrote the picture.
        Some(Err(e)) => {
            log::info!("no picture from the camera: {e}");
            Ok(None)
        }
        None => Err("could not read the picture".into()),
    }
}

const RECORD_AUDIO: &str = "android.permission.RECORD_AUDIO";

/// Ok if diari may use the microphone. If not, ask for it (Android shows
/// its dialog) and return an error telling the user to try again.
pub fn ensure_mic_permission() -> Result<(), String> {
    let app = current_app()?;
    let granted = with_activity(&app, |env, activity| {
        let perm = env.new_string(RECORD_AUDIO)?;
        let r = env
            .call_method(activity, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&perm)])?
            .i()?;
        Ok(r == 0) // PERMISSION_GRANTED
    })
    .ok_or("could not check the microphone permission")?;
    if granted {
        return Ok(());
    }
    with_activity(&app, |env, activity| {
        let perm = env.new_string(RECORD_AUDIO)?;
        let arr = env.new_object_array(1, "java/lang/String", &perm)?;
        env.call_method(activity, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(1)])?;
        Ok(())
    });
    Err("Allow diari to use the microphone, then tap the microphone again.".into())
}

/// Record 16-bit mono PCM at `rate` from the microphone until `stop` is
/// set. `started` is called once the microphone is running (or failed).
/// Blocks: run it on its own thread.
pub fn record_audio(
    rate: u32,
    stop: &AtomicBool,
    started: impl FnOnce(Result<(), String>),
) -> Result<Vec<i16>, String> {
    const VOICE_RECOGNITION: i32 = 6; // MediaRecorder.AudioSource, tuned for speech
    const CHANNEL_IN_MONO: i32 = 16;
    const ENCODING_PCM_16BIT: i32 = 2;
    const STATE_INITIALIZED: i32 = 1;

    let app = current_app()?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let mut started = Some(started);
    let mut fail = |env: &mut JNIEnv, e: String| {
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        if let Some(f) = started.take() {
            f(Err(e.clone()));
        }
        e
    };
    let rate_i = rate as i32;
    let min = env
        .call_static_method("android/media/AudioRecord", "getMinBufferSize", "(III)I", &[
            JValue::Int(rate_i),
            JValue::Int(CHANNEL_IN_MONO),
            JValue::Int(ENCODING_PCM_16BIT),
        ])
        .and_then(|v| v.i())
        .map_err(|e| e.to_string());
    let min = match min {
        Ok(m) if m > 0 => m,
        Ok(m) => return Err(fail(&mut env, format!("microphone not available ({m})"))),
        Err(e) => return Err(fail(&mut env, e)),
    };
    let buf_bytes = min.max(rate_i); // at least half a second
    let rec = match env.new_object("android/media/AudioRecord", "(IIIII)V", &[
        JValue::Int(VOICE_RECOGNITION),
        JValue::Int(rate_i),
        JValue::Int(CHANNEL_IN_MONO),
        JValue::Int(ENCODING_PCM_16BIT),
        JValue::Int(buf_bytes),
    ]) {
        Ok(r) => r,
        Err(e) => return Err(fail(&mut env, format!("microphone: {e}"))),
    };
    let state = env.call_method(&rec, "getState", "()I", &[]).and_then(|v| v.i()).unwrap_or(0);
    if state != STATE_INITIALIZED {
        let _ = env.call_method(&rec, "release", "()V", &[]);
        return Err(fail(&mut env, "the microphone is busy or not allowed".into()));
    }
    if let Err(e) = env.call_method(&rec, "startRecording", "()V", &[]) {
        let _ = env.call_method(&rec, "release", "()V", &[]);
        return Err(fail(&mut env, format!("microphone: {e}")));
    }
    if let Some(f) = started.take() {
        f(Ok(()));
    }

    let chunk = (rate_i / 10).max(256); // 100 ms
    let arr = env.new_short_array(chunk).map_err(|e| e.to_string())?;
    let mut pcm: Vec<i16> = Vec::with_capacity(rate as usize * 60);
    let mut tmp = vec![0i16; chunk as usize];
    let max = rate as usize * crate::voice::MAX_SECS as usize;
    let mut result = Ok(());
    while !stop.load(Ordering::SeqCst) && pcm.len() < max {
        let n = match env
            .call_method(&rec, "read", "([SII)I", &[JValue::Object(&arr), JValue::Int(0), JValue::Int(chunk)])
            .and_then(|v| v.i())
        {
            Ok(n) => n,
            Err(e) => {
                result = Err(format!("microphone: {e}"));
                break;
            }
        };
        if n < 0 {
            result = Err(format!("microphone read error {n}"));
            break;
        }
        if n > 0 {
            env.get_short_array_region(&arr, 0, &mut tmp[..n as usize]).map_err(|e| e.to_string())?;
            pcm.extend_from_slice(&tmp[..n as usize]);
        }
    }
    let _ = env.call_method(&rec, "stop", "()V", &[]);
    let _ = env.call_method(&rec, "release", "()V", &[]);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map(|()| pcm)
}

#[cfg(test)]
mod tests {
    use super::combine_share;

    #[test]
    fn share_text() {
        assert_eq!(combine_share("", "hello"), "hello");
        assert_eq!(combine_share("Title", "https://x.y"), "Title https://x.y");
        assert_eq!(combine_share("x.y", "see https://x.y"), "see https://x.y");
        assert_eq!(combine_share("Only", ""), "Only");
    }
}
