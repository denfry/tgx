//! Self-update from GitHub Releases (spec §12).
//!
//! A release is applied only when its zip carries a valid minisign signature
//! from the key compiled in below and the signed comment names exactly the
//! version being installed, so a renamed or older signed zip is refused.
//! The swap renames the running files to `*.old` (Windows allows renaming a
//! running exe and its loaded DLLs) and moves the new ones in; the `.old`
//! files are removed on the next start.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

const REPO_LATEST: &str = "https://api.github.com/repos/denfry/tgx/releases/latest";
/// Public half of the release signing key (`packaging/tgx-release.pub`).
const RELEASE_PUBKEY: &str = "RWRm9qtvBisq6pV4eX3sghKBFKBmQmvYUFaxQTc5ZJdEWTiM7wpGPmFt";
const MAX_ZIP: u64 = 200 * 1024 * 1024;
const MAX_SIG: u64 = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut it = s.split('.').map(|p| p.parse::<u64>().ok());
        let v = Version(it.next()??, it.next()??, it.next()??);
        it.next().is_none().then_some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is x.y.z")
}

fn asset_name(v: Version) -> String {
    format!("tgx-{v}-windows-x64.zip")
}

/// The trusted comment CI signs with; binds the signature to one version.
fn signed_comment(v: Version) -> String {
    format!("tgx {v} windows-x64")
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
}

pub struct Available {
    pub version: Version,
    zip_url: String,
    sig_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(concat!("tgx-updater/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Asks GitHub for the latest release; `None` when there is nothing newer.
pub fn check() -> Result<Option<Available>, String> {
    let release: Release = agent()
        .get(REPO_LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("проверка обновлений: {e}"))?
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json()
        .map_err(|e| format!("ответ GitHub: {e}"))?;
    pick(&release, current())
}

fn pick(release: &Release, current: Version) -> Result<Option<Available>, String> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::parse(&release.tag_name) else {
        return Ok(None);
    };
    if version <= current {
        return Ok(None);
    }
    let name = asset_name(version);
    let find = |n: &str| release.assets.iter().find(|a| a.name == n);
    let zip = find(&name).ok_or("в релизе нет архива для Windows")?;
    let sig = find(&format!("{name}.minisig")).ok_or("в релизе нет подписи")?;
    if zip.size > MAX_ZIP || sig.size > MAX_SIG {
        return Err("файлы релиза больше допустимого".into());
    }
    Ok(Some(Available {
        version,
        zip_url: zip.browser_download_url.clone(),
        sig_url: sig.browser_download_url.clone(),
    }))
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).call().map_err(|e| format!("загрузка: {e}"))?;
    let mut buf = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("загрузка: {e}"))?;
    if buf.len() as u64 > limit {
        return Err("файл больше допустимого".into());
    }
    Ok(buf)
}

/// Checks the signature and that it was made for exactly `version`.
pub fn verify(zip: &[u8], sig: &str, version: Version) -> Result<(), String> {
    verify_with(RELEASE_PUBKEY, zip, sig, version)
}

fn verify_with(pubkey: &str, zip: &[u8], sig: &str, version: Version) -> Result<(), String> {
    let pk = PublicKey::from_base64(pubkey).map_err(|e| format!("ключ: {e}"))?;
    let sig = Signature::decode(sig).map_err(|e| format!("подпись: {e}"))?;
    pk.verify(zip, &sig, false)
        .map_err(|_| "подпись архива не совпадает — обновление отклонено".to_string())?;
    if sig.trusted_comment() != signed_comment(version) {
        return Err("подпись сделана для другой версии — обновление отклонено".into());
    }
    Ok(())
}

/// Downloads, verifies and installs `a` next to the running exe. The new
/// version starts on the next launch.
pub fn install(a: &Available) -> Result<(), String> {
    let sig = download(&a.sig_url, MAX_SIG)?;
    let sig = String::from_utf8(sig).map_err(|_| "подпись не текстовая".to_string())?;
    let zip = download(&a.zip_url, MAX_ZIP)?;
    verify(&zip, &sig, a.version)?;
    let dir = install_dir()?;
    let files = unpack(&zip, a.version)?;
    swap_in(&dir, &files).map_err(|e| format!("установка: {e}"))
}

pub fn install_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| "не найдена папка программы".to_string())
}

/// Files of the release (path relative to the install dir, contents). Only
/// entries under the archive's top folder are taken, and only safe paths.
fn unpack(zip: &[u8], v: Version) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let prefix = PathBuf::from(format!("tgx-{v}-windows-x64"));
    let mut ar = zip::ZipArchive::new(io::Cursor::new(zip)).map_err(|e| format!("архив: {e}"))?;
    let mut out = Vec::new();
    for i in 0..ar.len() {
        let mut f = ar.by_index(i).map_err(|e| format!("архив: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let Some(path) = f.enclosed_name() else {
            return Err("в архиве недопустимый путь".into());
        };
        let Ok(rel) = path.strip_prefix(&prefix) else {
            return Err("в архиве файл вне папки релиза".into());
        };
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data).map_err(|e| format!("архив: {e}"))?;
        out.push((rel.to_path_buf(), data));
    }
    if !out.iter().any(|(p, _)| p == Path::new("tgx.exe")) {
        return Err("в архиве нет tgx.exe".into());
    }
    Ok(out)
}

fn old_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Moves the current files aside and writes the new ones; on any error the
/// files already replaced are put back.
fn swap_in(dir: &Path, files: &[(PathBuf, Vec<u8>)]) -> io::Result<()> {
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let result = (|| {
        for (rel, data) in files {
            let target = dir.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let existed = target.exists();
            if existed {
                let old = old_path(&target);
                let _ = fs::remove_file(&old);
                fs::rename(&target, &old)?;
            }
            done.push((target.clone(), existed));
            let tmp = old_path(&target).with_extension("new");
            fs::write(&tmp, data)?;
            fs::rename(&tmp, &target)?;
        }
        Ok(())
    })();
    if result.is_err() {
        for (target, existed) in done.iter().rev() {
            let _ = fs::remove_file(target);
            if *existed {
                let _ = fs::rename(old_path(target), target);
            }
        }
    }
    result
}

/// Removes `*.old` files left by the previous update.
pub fn cleanup_old(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            cleanup_old(&p);
        } else if p.extension().is_some_and(|x| x == "old") {
            let _ = fs::remove_file(p);
        }
    }
}

/// Starts the installed exe again and lets the caller exit.
pub fn restart() -> io::Result<()> {
    let exe = install_dir().map_err(io::Error::other)?.join("tgx.exe");
    std::process::Command::new(exe).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_order() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.10.0"), Some(Version(0, 10, 0)));
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2.x"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
    }

    fn release(tag: &str, assets: &[(&str, u64)]) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|(n, s)| Asset {
                    name: (*n).into(),
                    size: *s,
                    browser_download_url: format!("https://example.invalid/{n}"),
                })
                .collect(),
        }
    }

    #[test]
    fn pick_only_newer_with_both_assets() {
        let cur = Version(0, 1, 0);
        let full = [("tgx-0.2.0-windows-x64.zip", 10), ("tgx-0.2.0-windows-x64.zip.minisig", 300)];
        assert!(pick(&release("v0.2.0", &full), cur).unwrap().is_some());
        assert!(pick(&release("v0.1.0", &full), cur).unwrap().is_none());
        assert!(pick(&release("v0.0.9", &full), cur).unwrap().is_none());
        assert!(pick(&release("v0.2.0", &full[..1]), cur).is_err());
        let big = [("tgx-0.2.0-windows-x64.zip", MAX_ZIP + 1), ("tgx-0.2.0-windows-x64.zip.minisig", 300)];
        assert!(pick(&release("v0.2.0", &big), cur).is_err());
        let mut pre = release("v0.2.0", &full);
        pre.prerelease = true;
        assert!(pick(&pre, cur).unwrap().is_none());
    }

    // Throwaway key pair made for these tests; the release key is never in the repo.
    const TEST_PUB: &str = "RWQDoI2zgoozWwLXqTgtZgkBKtp2t1glJHC8cyweYHPvKBaqxWjnlwUr";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key
RUQDoI2zgoozWxbm9PLcKU8cg+oqqu2vddF/j+yNdQBU+5uuaOhTIA3ddyeX8dqVHRsGYbhe6m9niCbaGMnYhe7pRbguwVM1Ng4=
trusted comment: tgx 0.2.0 windows-x64
J8K0zQ40cVlDiWcI0epdHP8USE48Ttq6FuHXhkCvid8LUw3myrh+9pH6p5GCfsOpply/Uuzw5vPCWHSaBly/BQ==
";

    #[test]
    fn signature_checks() {
        let v = Version(0, 2, 0);
        assert!(verify_with(TEST_PUB, b"payload", TEST_SIG, v).is_ok());
        // Tampered file, other version, other key, garbage.
        assert!(verify_with(TEST_PUB, b"payloaX", TEST_SIG, v).is_err());
        assert!(verify_with(TEST_PUB, b"payload", TEST_SIG, Version(0, 3, 0)).is_err());
        assert!(verify(b"payload", TEST_SIG, v).is_err());
        assert!(verify(b"payload", "not a signature", v).is_err());
    }

    #[test]
    fn swap_in_replaces_and_keeps_old() {
        let dir = std::env::temp_dir().join(format!("tgx-upd-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tgx.exe"), b"old").unwrap();
        let files = vec![
            (PathBuf::from("tgx.exe"), b"new".to_vec()),
            (PathBuf::from("LICENSES/a.txt"), b"lic".to_vec()),
        ];
        swap_in(&dir, &files).unwrap();
        assert_eq!(fs::read(dir.join("tgx.exe")).unwrap(), b"new");
        assert_eq!(fs::read(dir.join("tgx.exe.old")).unwrap(), b"old");
        assert_eq!(fs::read(dir.join("LICENSES/a.txt")).unwrap(), b"lic");
        cleanup_old(&dir);
        assert!(!dir.join("tgx.exe.old").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
