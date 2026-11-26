//! Android glue over JNI: the intent perb was started with (share or
//! quick-add shortcut), the long-press "Quick add" shortcut, and finishing
//! the activity.

use jni::objects::{JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use slint::android::AndroidApp;

/// Action of the "Quick add" launcher shortcut.
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

/// If perb was started to quick-add a line, the text to prefill: the shared
/// text (with its subject, e.g. a page title), or empty for the shortcut.
pub fn launch_quick_add(app: &AndroidApp) -> Option<String> {
    with_activity(app, |env, activity| {
        let intent = env.call_method(activity, "getIntent", "()Landroid/content/Intent;", &[])?.l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let action = env.call_method(&intent, "getAction", "()Ljava/lang/String;", &[])?.l()?;
        match to_string(env, action)?.as_deref() {
            Some("android.intent.action.SEND") => {
                let text = string_extra(env, &intent, "android.intent.extra.TEXT")?.unwrap_or_default();
                let subject = string_extra(env, &intent, "android.intent.extra.SUBJECT")?.unwrap_or_default();
                Ok(Some(combine_share(&subject, &text)))
            }
            Some(QUICK_ADD_ACTION) => Ok(Some(String::new())),
            _ => Ok(None),
        }
    })
    .flatten()
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
