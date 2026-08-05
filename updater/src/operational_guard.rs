//! Conservative host-resource gates for unattended update work.
//!
//! All gates are opt-in. Unknown host state never blocks an update; a known
//! unsuitable condition records a clean deferral for the next scheduled run.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::{fs, path::Path};

use crate::{cache_cleanup, config::RuntimePaths};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GuardConfig {
    min_free_bytes: u64,
    defer_on_battery: bool,
    defer_on_metered_network: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    update_guards: GuardConfig,
}

pub fn deferral_reason(paths: &RuntimePaths, workspace_root: &Path) -> Result<Option<String>> {
    let config = load(paths)?;
    if config.min_free_bytes > 0 {
        let available = cache_cleanup::available_disk_bytes(workspace_root)?;
        if available < config.min_free_bytes {
            return Ok(Some(format!("update deferred: only {available} bytes free; update_guards.min_free_bytes requires {} bytes", config.min_free_bytes)));
        }
    }
    if config.defer_on_battery && on_battery_power() {
        return Ok(Some(
            "update deferred: system is running on battery power".to_string(),
        ));
    }
    if config.defer_on_metered_network && metered_network() {
        return Ok(Some(
            "update deferred: active network is metered".to_string(),
        ));
    }
    Ok(None)
}

fn load(paths: &RuntimePaths) -> Result<GuardConfig> {
    if !paths.config_file.exists() {
        return Ok(GuardConfig::default());
    }
    let content = fs::read_to_string(&paths.config_file)
        .with_context(|| format!("Failed to read {}", paths.config_file.display()))?;
    Ok(toml::from_str::<ConfigFile>(&content)
        .with_context(|| {
            format!(
                "Invalid update guard configuration in {}",
                paths.config_file.display()
            )
        })?
        .update_guards)
}

fn on_battery_power() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let (mut battery, mut mains) = (false, false);
    for entry in entries.flatten() {
        let path = entry.path();
        match fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .trim()
        {
            "Battery" => battery = true,
            "Mains" | "USB" | "USB_C" | "Wireless" => {
                mains |= fs::read_to_string(path.join("online"))
                    .map(|v| v.trim() == "1")
                    .unwrap_or(false)
            }
            _ => {}
        }
    }
    battery && !mains
}

fn metered_network() -> bool {
    let command = Path::new("/usr/bin/nmcli");
    command.is_file()
        && std::process::Command::new(command)
            .args(["-t", "-f", "GENERAL.METERED", "device", "show"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim_end().ends_with(":yes"))
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn paths(root: &Path) -> RuntimePaths {
        RuntimePaths {
            config_file: root.join("config.toml"),
            state_file: root.join("state.json"),
            log_file: root.join("log"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            config_dir: root.join("config"),
        }
    }
    #[test]
    fn minimum_disk_space_defers_before_work() -> Result<()> {
        let temp = TempDir::new()?;
        fs::write(
            temp.path().join("config.toml"),
            "[update_guards]\nmin_free_bytes = 18446744073709551615\n",
        )?;
        assert!(deferral_reason(&paths(temp.path()), temp.path())?
            .expect("must defer")
            .contains("min_free_bytes"));
        Ok(())
    }
    #[test]
    fn defaults_do_not_defer() -> Result<()> {
        let temp = TempDir::new()?;
        assert_eq!(deferral_reason(&paths(temp.path()), temp.path())?, None);
        Ok(())
    }
}
