//! SSH key management and git2 callbacks (credentials + trust-on-first-use host keys).

use git2::{CertificateCheckStatus, Cred, CredentialType, RemoteCallbacks};
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const PRIV: &str = "id_ed25519";
const PUB: &str = "id_ed25519.pub";
const KNOWN_HOSTS: &str = "known_hosts.toml";

#[derive(Clone)]
pub struct Keys {
    pub private_openssh: String,
    pub public_openssh: String,
    pub fingerprint: String,
}

impl Keys {
    pub fn load_or_generate(data_dir: &Path, comment: &str) -> Result<Self, String> {
        let p = data_dir.join(PRIV);
        if let Ok(s) = std::fs::read_to_string(&p) {
            match PrivateKey::from_openssh(&s) {
                Ok(k) => return Self::from_key(&k),
                Err(e) => log::warn!("cannot parse {}: {e}; regenerating", p.display()),
            }
        }
        Self::generate(data_dir, comment)
    }

    pub fn generate(data_dir: &Path, comment: &str) -> Result<Self, String> {
        let mut key = PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519).map_err(|e| e.to_string())?;
        key.set_comment(comment);
        let keys = Self::from_key(&key)?;
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        let priv_path = data_dir.join(PRIV);
        crate::notes::atomic_write(&priv_path, keys.private_openssh.as_bytes()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600));
        }
        crate::notes::atomic_write(&data_dir.join(PUB), keys.public_openssh.as_bytes()).map_err(|e| e.to_string())?;
        log::info!("generated new SSH key {}", keys.fingerprint);
        Ok(keys)
    }

    fn from_key(k: &PrivateKey) -> Result<Self, String> {
        let private_openssh = k.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?.to_string();
        let public_openssh = k.public_key().to_openssh().map_err(|e| e.to_string())?;
        let fingerprint = k.public_key().fingerprint(HashAlg::Sha256).to_string();
        Ok(Self { private_openssh, public_openssh, fingerprint })
    }
}

/// Trust-on-first-use store of SSH host key fingerprints.
pub struct KnownHosts {
    path: PathBuf,
    hosts: Mutex<BTreeMap<String, String>>,
}

impl KnownHosts {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(KNOWN_HOSTS);
        let hosts = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, hosts: Mutex::new(hosts) }
    }

    /// Ok if the fingerprint matches the stored one, or if the host is new
    /// (in which case it is stored). Err on mismatch.
    pub fn check(&self, host: &str, fp: &str) -> Result<(), String> {
        let mut hosts = self.hosts.lock().unwrap();
        match hosts.get(host) {
            Some(known) if known == fp => Ok(()),
            Some(known) => Err(format!(
                "Host key for {host} changed! Expected {known}, got {fp}. \
                 If this is expected, remove the entry from {}",
                self.path.display()
            )),
            None => {
                log::info!("trusting new host key for {host}: {fp}");
                hosts.insert(host.to_string(), fp.to_string());
                if let Ok(s) = toml::to_string(&*hosts) {
                    let _ = crate::notes::atomic_write(&self.path, s.as_bytes());
                }
                Ok(())
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build callbacks for fetch/push. `keys` may be None for local (file://) remotes.
pub fn callbacks<'a>(keys: &'a Keys, known: &'a KnownHosts) -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    let mut attempts = 0;
    cb.credentials(move |_url, username, allowed| {
        attempts += 1;
        if attempts > 3 {
            return Err(git2::Error::from_str("SSH authentication failed: is the public key added to the remote?"));
        }
        if allowed.contains(CredentialType::USERNAME) {
            return Cred::username(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::SSH_KEY) {
            return Cred::ssh_key_from_memory(
                username.unwrap_or("git"),
                Some(&keys.public_openssh),
                &keys.private_openssh,
                None,
            );
        }
        Err(git2::Error::from_str("remote requires an unsupported auth method (only SSH keys are supported)"))
    });
    cb.certificate_check(move |cert, host| {
        let Some(hk) = cert.as_hostkey() else {
            return Ok(CertificateCheckStatus::CertificatePassthrough);
        };
        let fp = if let Some(h) = hk.hash_sha256() {
            format!("SHA256:{}", hex(h))
        } else if let Some(h) = hk.hash_sha1() {
            format!("SHA1:{}", hex(h))
        } else {
            return Err(git2::Error::from_str("host key has no usable fingerprint"));
        };
        known
            .check(host, &fp)
            .map(|_| CertificateCheckStatus::CertificateOk)
            .map_err(|e| git2::Error::from_str(&e))
    });
    cb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_roundtrip_and_tofu() {
        let tmp = tempfile::tempdir().unwrap();
        let k1 = Keys::load_or_generate(tmp.path(), "diari@test").unwrap();
        assert!(k1.public_openssh.starts_with("ssh-ed25519 "));
        assert!(k1.private_openssh.contains("OPENSSH PRIVATE KEY"));
        let k2 = Keys::load_or_generate(tmp.path(), "diari@test").unwrap();
        assert_eq!(k1.public_openssh, k2.public_openssh);

        let kh = KnownHosts::load(tmp.path());
        assert!(kh.check("github.com", "SHA256:aa").is_ok());
        assert!(kh.check("github.com", "SHA256:aa").is_ok());
        let kh = KnownHosts::load(tmp.path());
        assert!(kh.check("github.com", "SHA256:bb").is_err());
    }
}
