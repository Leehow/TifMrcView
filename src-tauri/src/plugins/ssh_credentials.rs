//! Passwords live only in the current user's system credential store.
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{bail, Result};

fn service() -> String {
    format!("{}.ssh-password", crate::edition::APP_ID)
}

pub fn account(host: &str, port: u64, username: &str) -> Result<String> {
    if host.trim().is_empty() || username.is_empty() || !(1..=65535).contains(&port) {
        bail!("请填写服务器、端口和用户名");
    }
    Ok(super::digest(
        serde_json::to_string(&(host.trim().to_lowercase(), port, username))?.as_bytes(),
    ))
}

static STORE_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn load(account: &str) -> Result<Option<String>> {
    let _guard = STORE_ACCESS.lock().unwrap();
    platform::load(&service(), account)
}
pub fn save(account: &str, password: &str) -> Result<()> {
    let _guard = STORE_ACCESS.lock().unwrap();
    platform::save(&service(), account, password)
}
pub fn delete(account: &str) -> Result<()> {
    let _guard = STORE_ACCESS.lock().unwrap();
    platform::delete(&service(), account)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    pub fn load(service: &str, account: &str) -> Result<Option<String>> {
        match get_generic_password(service, account) {
            Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
            Err(e) if e.code() == -25300 => Ok(None),
            Err(_) => bail!("无法读取系统钥匙串中的 SSH 密码"),
        }
    }
    pub fn save(service: &str, account: &str, password: &str) -> Result<()> {
        set_generic_password(service, account, password.as_bytes())
            .map_err(|_| anyhow::anyhow!("无法保存 SSH 密码到系统钥匙串"))
    }
    pub fn delete(service: &str, account: &str) -> Result<()> {
        match delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == -25300 => Ok(()),
            Err(_) => bail!("无法删除系统钥匙串中的 SSH 密码"),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_NOT_FOUND},
        Security::Credentials::*,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    pub fn load(service: &str, account: &str) -> Result<Option<String>> {
        let target = wide(&format!("{service}/{account}"));
        unsafe {
            let mut ptr = std::ptr::null_mut();
            if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut ptr) == 0 {
                if GetLastError() == ERROR_NOT_FOUND {
                    return Ok(None);
                }
                bail!("无法读取 Windows 凭据管理器");
            }
            let bytes = if (*ptr).CredentialBlobSize == 0 {
                vec![]
            } else {
                std::slice::from_raw_parts(
                    (*ptr).CredentialBlob,
                    (*ptr).CredentialBlobSize as usize,
                )
                .to_vec()
            };
            CredFree(ptr.cast());
            Ok(Some(String::from_utf8(bytes)?))
        }
    }
    pub fn save(service: &str, account: &str, password: &str) -> Result<()> {
        let mut target = wide(&format!("{service}/{account}"));
        let mut user = wide(account);
        let mut bytes = password.as_bytes().to_vec();
        unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = target.as_mut_ptr();
            c.UserName = user.as_mut_ptr();
            c.CredentialBlobSize = bytes.len().try_into()?;
            c.CredentialBlob = bytes.as_mut_ptr();
            c.Persist = CRED_PERSIST_LOCAL_MACHINE;
            if CredWriteW(&c, 0) == 0 {
                bail!("无法保存 SSH 密码到 Windows 凭据管理器");
            }
        }
        Ok(())
    }
    pub fn delete(service: &str, account: &str) -> Result<()> {
        let target = wide(&format!("{service}/{account}"));
        unsafe {
            if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0
                && GetLastError() != ERROR_NOT_FOUND
            {
                bail!("无法删除 Windows 凭据管理器中的 SSH 密码");
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    fn command(action: &str, service: &str, account: &str) -> Command {
        let mut cmd = Command::new("secret-tool");
        cmd.arg(action);
        if action == "store" {
            cmd.arg("--label=TifMrcView SSH");
        }
        cmd.args(["service", service, "account", account]);
        cmd.stderr(Stdio::null());
        cmd
    }
    pub fn load(service: &str, account: &str) -> Result<Option<String>> {
        let out = command("lookup", service, account)
            .stderr(Stdio::piped())
            .output()
            .context("记住密码需要系统 Secret Service 和 secret-tool")?;
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(None);
        }
        if !out.status.success() {
            bail!("无法访问系统凭据库");
        }
        let mut bytes = out.stdout;
        // secret-tool adds one output newline; preserve whitespace in the password itself.
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        Ok(Some(String::from_utf8(bytes)?))
    }
    pub fn save(service: &str, account: &str, password: &str) -> Result<()> {
        if password.contains(['\n', '\r']) {
            bail!("系统凭据工具不支持含换行的密码");
        }
        let mut child = command("store", service, account)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("记住密码需要系统 Secret Service 和 secret-tool")?;
        child
            .stdin
            .take()
            .context("无法打开凭据库输入")?
            .write_all(password.as_bytes())?;
        if !child.wait()?.success() {
            bail!("无法保存 SSH 密码到系统凭据库");
        }
        Ok(())
    }
    pub fn delete(service: &str, account: &str) -> Result<()> {
        let out = command("clear", service, account)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("无法访问系统 Secret Service")?;
        if !out.status.success() && !(out.status.code() == Some(1) && out.stderr.is_empty()) {
            bail!("无法删除系统凭据库中的 SSH 密码");
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod platform {
    use super::*;
    pub fn load(_: &str, _: &str) -> Result<Option<String>> {
        bail!("当前系统不支持记住 SSH 密码")
    }
    pub fn save(_: &str, _: &str, _: &str) -> Result<()> {
        bail!("当前系统不支持记住 SSH 密码")
    }
    pub fn delete(_: &str, _: &str) -> Result<()> {
        bail!("当前系统不支持记住 SSH 密码")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolates_server_port_and_case_sensitive_user() {
        let a = account(" Host ", 22, "User").unwrap();
        assert_eq!(a, account("host", 22, "User").unwrap());
        for (h, p, u) in [
            ("other", 22, "User"),
            ("host", 23, "User"),
            ("host", 22, "user"),
        ] {
            assert_ne!(a, account(h, p, u).unwrap());
        }
        assert!(account("", 22, "user").is_err());
        assert!(account("host", 0, "user").is_err());
    }
}
