//! Asks GitHub for the latest PdfCraft release (Help ▸ Check for updates, issue #28).

use std::time::Duration;

use pdfcraft_ui_egui::updates::{Installer, RELEASES_PAGE, Release};

/// The Thai edition's latest release (Thai edition: its own repository, see [`RELEASES_PAGE`]).
// Bridge release (0.20.0-th.2): PdfCraft Thai Edition is now MaewPDF, a separate program with its
// own installer, and this release moves its users there: it offers MaewPDF's releases.
const LATEST: &str = "https://api.github.com/repos/mixlnwuu/maewpdf/releases/latest";

/// Where the edition's release files are: an installer listed anywhere else is not offered.
const DOWNLOADS: &str = "https://github.com/mixlnwuu/maewpdf/releases/download/";

/// The largest installer accepted (today's is about 100 MB).
const MAX_INSTALLER: u64 = 1 << 30;

/// The latest release. The answer is untrusted: its size is capped, and only a page under
/// [`RELEASES_PAGE`] is ever offered for download (anything else falls back to that list).
pub fn latest_release() -> Result<Release, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build())
        .build()
        .new_agent();
    let mut response = agent
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
        // Read error statuses ourselves: a 403 from GitHub usually means its anonymous
        // rate limit ran out, and the retry time rides along in a response header.
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
    let status = response.status().as_u16();
    if status == 403 {
        let reset = response.headers().get("x-ratelimit-reset").and_then(|v| v.to_str().ok());
        return Err(rate_limited(reset, now_unix()));
    }
    if !(200..300).contains(&status) {
        return Err(format!("couldn't reach GitHub (http status: {status})"));
    }
    let body = response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("unreadable answer ({e})"))?;
    parse(&body)
}

/// The certificate authorities the operating system trusts.
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates found on this system".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

/// The current Unix time in seconds (0 when the clock gives nothing usable).
fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// What to tell the user when GitHub answers 403: its anonymous quota (60 checks an hour,
/// shared by everyone behind the same address) ran out. `reset` is the `x-ratelimit-reset`
/// header, the Unix time the quota refills; `now` is the current Unix time. Anything
/// surprising (no header, not a number, already past) falls back to asking for patience.
fn rate_limited(reset: Option<&str>, now: u64) -> String {
    const FALLBACK: &str = "GitHub is limiting update checks for now (shared hourly quota); try again later";
    let wait = reset.and_then(|r| r.trim().parse::<u64>().ok()).and_then(|reset| reset.checked_sub(now)).filter(|&wait| wait > 0);
    match wait {
        Some(wait) => {
            let minutes = wait.div_ceil(60).max(1);
            let plural = if minutes == 1 { "" } else { "s" };
            format!("GitHub is limiting update checks for now (shared hourly quota); try again in about {minutes} minute{plural}")
        }
        None => FALLBACK.into(),
    }
}

fn parse(body: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("unreadable answer ({e})"))?;
    let version = v["tag_name"].as_str().filter(|t| !t.is_empty() && t.len() <= 64).ok_or("no release found")?.to_string();
    let url = v["html_url"]
        .as_str()
        .filter(|u| u.strip_prefix(RELEASES_PAGE).is_some_and(|rest| rest.starts_with('/') && !rest.contains(['?', '#', '\\'])))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    let installer = installer(&v, &version);
    Ok(Release { version, url, installer })
}

/// The release's Windows installer: named for `version`, on [`DOWNLOADS`], with GitHub's SHA-256
/// and a size that makes sense. Anything else is not offered.
fn installer(v: &serde_json::Value, version: &str) -> Option<Installer> {
    let name = format!("maewpdf-{}-windows-x64.msi", version.trim_start_matches(['v', 'V']));
    let asset = v["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(name.as_str()))?;
    let url = asset["browser_download_url"].as_str().filter(|u| u.starts_with(DOWNLOADS) && !u.contains(['?', '#']) && !u.contains(".."))?;
    let size = asset["size"].as_u64().filter(|s| (1..=MAX_INSTALLER).contains(s))?;
    let sha256 = asset["digest"].as_str()?.strip_prefix("sha256:").filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some(Installer { url: url.to_owned(), size, sha256: sha256.to_ascii_lowercase() })
}

/// Downloads `installer` into the temporary folder and checks its size and SHA-256 before the
/// file gets its `.msi` name. The checksum is GitHub's, so it catches a damaged or swapped
/// download, not a release published with someone else's hands on the account.
#[cfg(windows)]
pub fn download(installer: &Installer, progress: &(dyn Fn(f32) + Sync), stop: &std::sync::atomic::AtomicBool) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    if !installer.url.starts_with(DOWNLOADS) || installer.size == 0 || installer.size > MAX_INSTALLER {
        return Err("the installer isn't one PdfCraft can use".into());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(os_roots()?).build())
        .build()
        .new_agent();
    let dir = std::env::temp_dir().join("PdfCraft-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make a folder for the download ({e})"))?;
    let part = dir.join("pdfcraft-update.part");
    let msi = dir.join("pdfcraft-update.msi");
    let result = (|| {
        let mut response = agent
            .get(&installer.url)
            .header("User-Agent", concat!("PdfCraft/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
        let mut reader = response.body_mut().with_config().limit(installer.size.saturating_add(1)).reader();
        let mut file = std::fs::File::create(&part).map_err(|e| format!("couldn't save the download ({e})"))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 16];
        let mut read = 0u64;
        loop {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let n = reader.read(&mut buffer).map_err(|e| format!("the download stopped ({e})"))?;
            let Some(chunk) = buffer.get(..n).filter(|c| !c.is_empty()) else { break };
            hasher.update(chunk);
            file.write_all(chunk).map_err(|e| format!("couldn't save the download ({e})"))?;
            read = read.saturating_add(n as u64);
            progress((read as f64 / installer.size as f64) as f32);
        }
        file.flush().map_err(|e| format!("couldn't save the download ({e})"))?;
        drop(file);
        let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if read != installer.size || digest != installer.sha256 {
            return Err("the download doesn't match the release's checksum".to_string());
        }
        std::fs::rename(&part, &msi).map_err(|e| format!("couldn't save the download ({e})"))
    })();
    if result.is_err() {
        // Best effort: a leftover in the temporary folder harms nothing and the next try replaces it.
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| msi)
}

/// Runs `msi` once PdfCraft has closed, for this install's scope, then opens PdfCraft again. It
/// waits for this process to be gone: an installer that finds the program running can't replace
/// it, and the update would seem to have done nothing. Windows asks for permission where an
/// install for everyone needs it. The installer's log stays beside it.
#[cfg(windows)]
pub fn run(msi: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let plain = |p: &std::path::Path| {
        p.to_str().filter(|s| !s.contains(['"', '%'])).map(str::to_owned).ok_or_else(|| "a path PdfCraft can't pass on".to_string())
    };
    let package = plain(msi)?;
    let script = msi.with_extension("cmd");
    let log = plain(&msi.with_extension("log"))?;
    std::fs::write(&script, update_script(std::process::id(), &package, &log)).map_err(|e| format!("couldn't start the installer ({e})"))?;
    std::process::Command::new("cmd.exe")
        .raw_arg(format!("/s /c \"\"{}\"\"", plain(&script)?))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(drop)
        .map_err(|e| format!("couldn't start the installer ({e})"))
}

/// The batch file behind [`run`]: wait up to a minute for process `pid` to end, install MaewPDF
/// for this user (no administrator prompt), open it by its App Paths name. MaewPDF's first start
/// copies the settings and removes this edition (MaewPDF's `old_edition.rs`).
#[cfg(any(windows, test))]
fn update_script(pid: u32, msi: &str, log: &str) -> String {
    let scope = "ALLUSERS=2 MSIINSTALLPERUSER=1";
    format!(
        "@echo off\r\nchcp 65001 >nul\r\nset n=0\r\n:wait\r\ntasklist /fi \"PID eq {pid}\" /nh | find \"{pid}\" >nul\r\nif errorlevel 1 goto install\r\n\
         set /a n+=1\r\nif %n% lss 60 (\r\n  ping -n 2 127.0.0.1 >nul\r\n  goto wait\r\n)\r\n:install\r\n\
         start \"\" /wait msiexec.exe /i \"{msi}\" /passive /norestart {scope} /l*v \"{log}\"\r\nstart \"\" maewpdf.exe\r\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thai edition: the installer offered is this release's own, on the edition's downloads,
    /// with GitHub's checksum and a sensible size; anything else leaves the release page.
    #[test]
    fn only_a_checked_installer_of_this_release_is_offered() {
        let hash = "A".repeat(64);
        let answer = |name: &str, url: &str, size: u64, digest: &str| {
            format!(
                r#"{{"tag_name":"v1.2.3-th.1","html_url":"","assets":[{{"name":"{name}","browser_download_url":"{url}","size":{size},"digest":"{digest}"}}]}}"#
            )
        };
        let good = "https://github.com/mixlnwuu/maewpdf/releases/download/v1.2.3-th.1/maewpdf-1.2.3-th.1-windows-x64.msi";
        let name = "maewpdf-1.2.3-th.1-windows-x64.msi";
        let digest = format!("sha256:{hash}");
        let i = parse(&answer(name, good, 5, &digest)).unwrap().installer.expect("offered");
        assert_eq!((i.url.as_str(), i.size, i.sha256), (good, 5, "a".repeat(64)));
        for (name, url, size, digest) in [
            ("maewpdf-1.2.2-windows-x64.msi", good, 5, digest.as_str()),
            (name, "https://example.com/pdfcraft.msi", 5, digest.as_str()),
            (name, "https://github.com/storytold/pdfcraft/releases/download/v1/x.msi", 5, digest.as_str()),
            (name, "https://github.com/mixlnwuu/pdfcraft/releases/download/v1.2.3-th.1/maewpdf-1.2.3-th.1-windows-x64.msi", 5, digest.as_str()),
            (name, "https://github.com/mixlnwuu/maewpdf/releases/download/../../evil.msi", 5, digest.as_str()),
            (name, good, 0, digest.as_str()),
            (name, good, MAX_INSTALLER + 1, digest.as_str()),
            (name, good, 5, "sha256:short"),
            (name, good, 5, ""),
        ] {
            assert_eq!(parse(&answer(name, url, size, digest)).unwrap().installer, None, "{name} {url} {size} {digest}");
        }
    }

    /// Bridge release: MaewPDF's installer starts only after this app has gone, is waited for,
    /// installs for this user, and MaewPDF (not this edition) is opened after it.
    #[test]
    fn the_bridge_script_installs_maewpdf_after_closing_and_opens_it() {
        let script = update_script(42, r"C:\\T\\u.msi", r"C:\\T\\u.log");
        let wait = script.find("tasklist /fi \"PID eq 42\"").unwrap();
        let install = script
            .find(r#"start "" /wait msiexec.exe /i "C:\\T\\u.msi" /passive /norestart ALLUSERS=2 MSIINSTALLPERUSER=1 /l*v "C:\\T\\u.log""#)
            .expect("the installer, waited for");
        let open = script.find("start \"\" maewpdf.exe\r\n").expect("MaewPDF opens");
        assert!(wait < install && install < open, "{script}");
        assert!(!script.contains("pdfcraft.exe"), "this edition is not reopened: {script}");
    }

    #[test]
    fn answers_are_read_and_only_our_release_pages_are_offered() {
        let r = parse(r#"{"tag_name":"v0.2.0","html_url":"https://github.com/mixlnwuu/maewpdf/releases/tag/v0.2.0"}"#).unwrap();
        assert_eq!(r, Release { version: "v0.2.0".into(), url: "https://github.com/mixlnwuu/maewpdf/releases/tag/v0.2.0".into(), installer: None });
        // Upstream's release pages too (Thai edition): the check offers only the edition's.
        for elsewhere in [
            "https://example.com/pdfcraft.exe",
            "https://github.com/mixlnwuu/maewpdf/releases.evil/x",
            "https://github.com/mixlnwuu/pdfcraft/releases/tag/v9.9.9",
            "javascript:alert(1)",
            "https://github.com/storytold/pdfcraft/releases/tag/v9.9.9",
        ] {
            let r = parse(&format!(r#"{{"tag_name":"v9.9.9","html_url":"{elsewhere}"}}"#)).unwrap();
            assert_eq!(r.url, RELEASES_PAGE, "{elsewhere}");
        }
        assert!(parse(r#"{"message":"Not Found"}"#).is_err());
        assert!(parse("<html>").is_err());
    }

    #[test]
    fn rate_limit_messages_name_the_wait() {
        let limited = "GitHub is limiting update checks for now (shared hourly quota); try again in about 20 minutes";
        assert_eq!(rate_limited(Some("1800001200"), 1800000000), limited);
        // Rounds up: 61 seconds out reads as 2 minutes.
        assert_eq!(
            rate_limited(Some("1800000061"), 1800000000),
            "GitHub is limiting update checks for now (shared hourly quota); try again in about 2 minutes"
        );
        assert_eq!(
            rate_limited(Some("1800000060"), 1800000000),
            "GitHub is limiting update checks for now (shared hourly quota); try again in about 1 minute"
        );
        // No header, not a number, or already past: ask for patience instead of a time.
        let later = "GitHub is limiting update checks for now (shared hourly quota); try again later";
        for bad in [None, Some(""), Some("soon"), Some("1799999999"), Some("1800000000")] {
            assert_eq!(rate_limited(bad, 1800000000), later);
        }
    }

    /// Live: asks GitHub over TLS with the OS's roots (`cargo test -p pdfcraft -- --ignored`).
    #[test]
    #[ignore = "needs network access"]
    fn github_answers_with_the_latest_release() {
        let r = latest_release().unwrap();
        assert!(pdfcraft_ui_egui::updates::is_newer(&r.version, "0.0.0"), "{r:?}");
        assert!(r.url.starts_with(RELEASES_PAGE), "{r:?}");
    }
}
