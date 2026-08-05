//! Durable, idempotent notification delivery for the update manager.
//!
//! Desktop notifications are best effort. Email is different: an updater must
//! survive a restart between deciding to notify and handing the message to the
//! local MTA. This module therefore persists an outbox record before delivery.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{io::AsyncWriteExt, process::Command, time};

use crate::{config::RuntimePaths, state};

const OUTBOX_DIR: &str = "notifications";
const OUTBOX_LOCK: &str = ".lock";
const DEFAULT_MAX_ATTEMPTS: u32 = 6;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_RETRY_JITTER_SECONDS: u64 = 15;
const MAX_BACKOFF_SECONDS: i64 = 6 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationEvent {
    pub key: String,
    pub kind: String,
    pub summary: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct EmailConfigFile {
    enabled: bool,
    to: String,
    from: Option<String>,
    sendmail_command: Option<PathBuf>,
    sendmail_args: Vec<String>,
    timeout_seconds: Option<u64>,
    max_attempts: Option<u32>,
    retry_jitter_seconds: Option<u64>,
    quiet_hours_start_utc: Option<String>,
    quiet_hours_end_utc: Option<String>,
    /// `immediate` (default) or `digest`; digest holds noncritical mail until
    /// the configured UTC hour, then sends the durable individual events.
    delivery_mode: Option<String>,
    digest_hour_utc: Option<u8>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ConfigFile {
    email_notifications: EmailConfigFile,
}

#[derive(Debug, Clone)]
struct EmailConfig {
    to: String,
    from: Option<String>,
    command: PathBuf,
    args: Vec<String>,
    timeout_seconds: u64,
    max_attempts: u32,
    retry_jitter_seconds: u64,
    quiet_hours: Option<(u16, u16)>,
    digest_hour_utc: Option<u8>,
}

pub fn enqueue(
    paths: &RuntimePaths,
    key: impl Into<String>,
    kind: impl Into<String>,
    summary: impl Into<String>,
    body: impl Into<String>,
) -> Result<bool> {
    let key = key.into();
    validate_key(&key)?;
    let _lock = OutboxLock::acquire(paths)?;
    let path = event_path(paths, &key);
    if path.exists() {
        return Ok(false);
    }
    let now = Utc::now();
    let event = NotificationEvent {
        key,
        kind: kind.into(),
        summary: summary.into(),
        body: body.into(),
        created_at: now,
        updated_at: now,
        status: DeliveryStatus::Pending,
        attempts: 0,
        next_attempt_at: Some(now),
        last_error: None,
    };
    write_event(&path, &event)?;
    Ok(true)
}

pub fn list(paths: &RuntimePaths) -> Result<Vec<NotificationEvent>> {
    let dir = outbox_dir(paths);
    let mut events = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(events),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", dir.display()))
        }
    };
    for entry in entries {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            events.push(read_event(&entry.path())?);
        }
    }
    events.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(events)
}

pub fn retry(paths: &RuntimePaths, key: &str) -> Result<()> {
    let _lock = OutboxLock::acquire(paths)?;
    let path = event_path(paths, key);
    let mut event = read_event(&path)?;
    event.status = DeliveryStatus::Pending;
    event.next_attempt_at = Some(Utc::now());
    event.last_error = None;
    event.updated_at = Utc::now();
    write_event(&path, &event)
}

pub fn acknowledge(paths: &RuntimePaths, key: &str) -> Result<()> {
    let _lock = OutboxLock::acquire(paths)?;
    let path = event_path(paths, key);
    let mut event = read_event(&path)?;
    event.status = DeliveryStatus::Acknowledged;
    event.next_attempt_at = None;
    event.updated_at = Utc::now();
    write_event(&path, &event)
}

/// Delivers all due events. A crash during delivery leaves `sending` on disk;
/// the next run deliberately treats it as pending, preferring at-least-once
/// local MTA handoff over silently dropping an update notification.
pub async fn deliver_due(paths: &RuntimePaths) -> Result<usize> {
    let Some(config) = load_email_config(&paths.config_file)? else {
        return Ok(0);
    };
    let now = Utc::now();
    let _lock = OutboxLock::acquire(paths)?;
    let mut delivered = 0;
    for mut event in list(paths)? {
        if !is_due(&event, now) {
            continue;
        }
        if delivery_is_deferred(&config, &event, now) {
            continue;
        }
        let path = event_path(paths, &event.key);
        event.status = DeliveryStatus::Sending;
        event.updated_at = now;
        write_event(&path, &event)?;
        match send_email(&config, &event).await {
            Ok(()) => {
                event.status = DeliveryStatus::Sent;
                event.next_attempt_at = None;
                event.last_error = None;
                event.updated_at = Utc::now();
                write_event(&path, &event)?;
                delivered += 1;
            }
            Err(error) => {
                event.attempts = event.attempts.saturating_add(1);
                event.last_error = Some(redact_error(&error.to_string()));
                event.updated_at = Utc::now();
                if event.attempts >= config.max_attempts {
                    event.status = DeliveryStatus::Failed;
                    event.next_attempt_at = None;
                } else {
                    event.status = DeliveryStatus::Pending;
                    event.next_attempt_at =
                        Some(event.updated_at + retry_backoff(&event, config.retry_jitter_seconds));
                }
                write_event(&path, &event)?;
            }
        }
    }
    Ok(delivered)
}

fn is_due(event: &NotificationEvent, now: DateTime<Utc>) -> bool {
    match event.status {
        DeliveryStatus::Pending | DeliveryStatus::Sending => {
            event.next_attempt_at.is_none_or(|due| due <= now)
        }
        DeliveryStatus::Sent | DeliveryStatus::Failed | DeliveryStatus::Acknowledged => false,
    }
}

fn delivery_is_deferred(
    config: &EmailConfig,
    event: &NotificationEvent,
    now: DateTime<Utc>,
) -> bool {
    if is_critical(event) {
        return false;
    }
    if let Some((start, end)) = config.quiet_hours {
        let minute = (now.hour() as u16) * 60 + now.minute() as u16;
        let inside = if start < end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        };
        if inside {
            return true;
        }
    }
    config
        .digest_hour_utc
        .is_some_and(|hour| now.hour() as u8 != hour)
}

fn is_critical(event: &NotificationEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "build_failed" | "check_unhealthy" | "install_failed"
    )
}

fn retry_backoff(event: &NotificationEvent, jitter_max_seconds: u64) -> ChronoDuration {
    let exponent = event.attempts.saturating_sub(1).min(12);
    let seconds = (30_i64.saturating_mul(1_i64 << exponent)).min(MAX_BACKOFF_SECONDS);
    // Stable key-derived jitter avoids a thundering herd while remaining testable.
    let mut digest = Sha256::new();
    digest.update(event.key.as_bytes());
    digest.update(event.attempts.to_le_bytes());
    let jitter = i64::from(digest.finalize()[0]) % (jitter_max_seconds.min(300) as i64 + 1);
    ChronoDuration::seconds(seconds + jitter)
}

fn load_email_config(path: &Path) -> Result<Option<EmailConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&content).with_context(|| {
        format!(
            "Invalid email notification configuration in {}",
            path.display()
        )
    })?;
    let email = parsed.email_notifications;
    if !email.enabled {
        return Ok(None);
    }
    validate_mailbox("email_notifications.to", &email.to)?;
    if let Some(from) = email.from.as_deref() {
        validate_mailbox("email_notifications.from", from)?;
    }
    let command = email
        .sendmail_command
        .unwrap_or_else(|| PathBuf::from("/usr/sbin/sendmail"));
    validate_sendmail_command(&command)?;
    if email.sendmail_args.iter().any(|arg| arg.contains('\0')) {
        bail!("email_notifications.sendmail_args contains a NUL byte");
    }
    let quiet_hours = match (
        email.quiet_hours_start_utc.as_deref(),
        email.quiet_hours_end_utc.as_deref(),
    ) {
        (None, None) => None,
        (Some(start), Some(end)) => Some((
            parse_time("email_notifications.quiet_hours_start_utc", start)?,
            parse_time("email_notifications.quiet_hours_end_utc", end)?,
        )),
        _ => bail!("email notification quiet hours require both start and end UTC times"),
    };
    let digest_hour_utc = match email.delivery_mode.as_deref().unwrap_or("immediate") {
        "immediate" => None,
        "digest" => Some(email.digest_hour_utc.unwrap_or(9)),
        _ => bail!("email_notifications.delivery_mode must be `immediate` or `digest`"),
    };
    if digest_hour_utc.is_some_and(|hour| hour > 23) {
        bail!("email_notifications.digest_hour_utc must be between 0 and 23");
    }
    Ok(Some(EmailConfig {
        to: email.to,
        from: email.from,
        command,
        args: if email.sendmail_args.is_empty() {
            vec!["-t".to_string(), "-i".to_string()]
        } else {
            email.sendmail_args
        },
        timeout_seconds: email
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
            .clamp(1, 300),
        max_attempts: email
            .max_attempts
            .unwrap_or(DEFAULT_MAX_ATTEMPTS)
            .clamp(1, 20),
        retry_jitter_seconds: email
            .retry_jitter_seconds
            .unwrap_or(DEFAULT_RETRY_JITTER_SECONDS)
            .min(300),
        quiet_hours,
        digest_hour_utc,
    }))
}

fn parse_time(field: &str, value: &str) -> Result<u16> {
    let mut parts = value.split(':');
    let hour = parts.next().and_then(|part| part.parse::<u16>().ok());
    let minute = parts.next().and_then(|part| part.parse::<u16>().ok());
    if parts.next().is_some()
        || hour.is_none_or(|hour| hour > 23)
        || minute.is_none_or(|minute| minute > 59)
    {
        bail!("{field} must use HH:MM in UTC");
    }
    Ok(hour.unwrap() * 60 + minute.unwrap())
}

fn validate_mailbox(field: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.contains(['\r', '\n', '\0']) || !value.contains('@') {
        bail!("{field} must be a single valid email address");
    }
    Ok(())
}

fn validate_sendmail_command(command: &Path) -> Result<()> {
    anyhow::ensure!(
        command.is_absolute(),
        "email_notifications.sendmail_command must be an absolute path"
    );
    let metadata = fs::metadata(command)
        .with_context(|| format!("Unable to inspect sendmail command {}", command.display()))?;
    anyhow::ensure!(
        metadata.is_file() && metadata.mode() & 0o111 != 0,
        "sendmail command {} is not executable",
        command.display()
    );
    anyhow::ensure!(
        metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "sendmail command {} must be root-owned and not group/world writable",
        command.display()
    );
    Ok(())
}

async fn send_email(config: &EmailConfig, event: &NotificationEvent) -> Result<()> {
    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // A wedged local MTA must not survive the bounded delivery attempt.
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start {}", config.command.display()))?;
    let message = format_email(config, event);
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(message.as_bytes())
            .await
            .context("Failed to write email message")?;
        stdin
            .shutdown()
            .await
            .context("Failed to close email message")?;
    }
    let output = time::timeout(
        std::time::Duration::from_secs(config.timeout_seconds),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "sendmail timed out after {} seconds",
            config.timeout_seconds
        )
    })?
    .context("Failed to wait for sendmail")?;
    if !output.status.success() {
        bail!(
            "sendmail exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn format_email(config: &EmailConfig, event: &NotificationEvent) -> String {
    let from = config.from.as_deref().unwrap_or("codex-desktop@localhost");
    let subject = event.summary.replace(['\r', '\n'], " ");
    format!("To: {}\nFrom: {}\nSubject: {}\nAuto-Submitted: auto-generated\nX-Codex-Update-Event: {}\nContent-Type: text/plain; charset=UTF-8\n\n{}\n\nEvent key: {}\n", config.to, from, subject, event.kind, event.body, event.key)
}

fn redact_error(value: &str) -> String {
    // Credentials should live in the MTA configuration, never this process. Do
    // not preserve arbitrary MTA output, which could nevertheless include one.
    let _ = value;
    "sendmail delivery failed; inspect the local MTA log".to_string()
}

fn validate_key(key: &str) -> Result<()> {
    anyhow::ensure!(
        !key.trim().is_empty() && key.len() <= 512 && !key.contains('\0'),
        "notification event key is invalid"
    );
    Ok(())
}

fn outbox_dir(paths: &RuntimePaths) -> PathBuf {
    paths.state_dir.join(OUTBOX_DIR)
}

struct OutboxLock {
    _file: fs::File,
}

impl OutboxLock {
    fn acquire(paths: &RuntimePaths) -> Result<Self> {
        let dir = outbox_dir(paths);
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        let path = dir.join(OUTBOX_LOCK);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("Failed to inspect {}", path.display()))?;
        anyhow::ensure!(
            metadata.is_file() && metadata.uid() == unsafe { libc::geteuid() },
            "Notification outbox lock {} is not a user-owned regular file",
            path.display()
        );
        if metadata.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to secure {}", path.display()))?;
        }
        file.lock()
            .with_context(|| format!("Failed to lock {}", path.display()))?;
        Ok(Self { _file: file })
    }
}

fn event_path(paths: &RuntimePaths, key: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    let file_name: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    outbox_dir(paths).join(format!("{file_name}.json"))
}

fn read_event(path: &Path) -> Result<NotificationEvent> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read notification event {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Invalid notification event {}", path.display()))
}

fn write_event(path: &Path, event: &NotificationEvent) -> Result<()> {
    state::atomic_write(path, serde_json::to_string_pretty(event)?.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn paths(root: &Path) -> RuntimePaths {
        RuntimePaths {
            config_file: root.join("config/config.toml"),
            state_file: root.join("state/state.json"),
            log_file: root.join("state/service.log"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            config_dir: root.join("config"),
        }
    }

    #[test]
    fn enqueue_is_idempotent_and_events_are_private() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        assert!(enqueue(
            &paths,
            "dmg:abc",
            "update_available",
            "subject",
            "body"
        )?);
        assert!(!enqueue(
            &paths,
            "dmg:abc",
            "update_available",
            "other",
            "other"
        )?);
        let events = list(&paths)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, "dmg:abc");
        let metadata = fs::metadata(event_path(&paths, "dmg:abc"))?;
        assert_eq!(metadata.mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn concurrent_enqueue_creates_one_event() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        let left = paths.clone();
        let right = paths.clone();
        let left_thread = std::thread::spawn(move || {
            enqueue(
                &left,
                "dmg:concurrent",
                "update_available",
                "subject",
                "left",
            )
        });
        let right_thread = std::thread::spawn(move || {
            enqueue(
                &right,
                "dmg:concurrent",
                "update_available",
                "subject",
                "right",
            )
        });
        let left_inserted = left_thread.join().expect("left enqueue thread")?;
        let right_inserted = right_thread.join().expect("right enqueue thread")?;
        assert_ne!(left_inserted, right_inserted);
        assert_eq!(list(&paths)?.len(), 1);
        Ok(())
    }

    #[test]
    fn retry_and_acknowledge_are_durable() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        enqueue(&paths, "failure:abc", "failed", "subject", "body")?;
        acknowledge(&paths, "failure:abc")?;
        assert_eq!(list(&paths)?[0].status, DeliveryStatus::Acknowledged);
        retry(&paths, "failure:abc")?;
        assert_eq!(list(&paths)?[0].status, DeliveryStatus::Pending);
        Ok(())
    }

    #[test]
    fn stable_backoff_is_bounded() {
        let event = NotificationEvent {
            key: "a".into(),
            kind: "a".into(),
            summary: "a".into(),
            body: "a".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: DeliveryStatus::Pending,
            attempts: 100,
            next_attempt_at: None,
            last_error: None,
        };
        assert!(retry_backoff(&event, 15).num_seconds() <= MAX_BACKOFF_SECONDS + 15);
    }

    #[test]
    fn quiet_hours_and_digest_defer_only_noncritical_events() {
        let now = Utc::now();
        let normal = NotificationEvent {
            key: "normal".into(),
            kind: "update_available".into(),
            summary: "s".into(),
            body: "b".into(),
            created_at: now,
            updated_at: now,
            status: DeliveryStatus::Pending,
            attempts: 0,
            next_attempt_at: Some(now),
            last_error: None,
        };
        let critical = NotificationEvent {
            kind: "build_failed".into(),
            ..normal.clone()
        };
        let config = EmailConfig {
            to: "a@b".into(),
            from: None,
            command: PathBuf::from("/usr/bin/true"),
            args: vec![],
            timeout_seconds: 1,
            max_attempts: 1,
            retry_jitter_seconds: 0,
            quiet_hours: Some((0, 0)),
            digest_hour_utc: Some((now.hour() as u8 + 1) % 24),
        };
        // Equal boundaries cover the full day, and critical failures bypass it.
        assert!(delivery_is_deferred(&config, &normal, now));
        assert!(!delivery_is_deferred(&config, &critical, now));
    }

    #[test]
    fn quiet_time_validation_rejects_invalid_values() {
        assert!(parse_time("x", "24:00").is_err());
        assert!(parse_time("x", "12:60").is_err());
        assert_eq!(parse_time("x", "09:05").unwrap(), 545);
    }

    #[tokio::test]
    async fn deliver_uses_root_owned_fake_transport_and_recovers_sending_event() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        let delivered_mail = temp.path().join("delivered-mail.txt");
        fs::create_dir_all(&paths.config_dir)?;
        fs::write(
            &paths.config_file,
            format!(
                "[email_notifications]\nenabled = true\nto = \"test@example.invalid\"\nfrom = \"updater@example.invalid\"\nsendmail_command = \"/usr/bin/tee\"\nsendmail_args = [\"{}\"]\n",
                delivered_mail.display()
            ),
        )?;
        enqueue(&paths, "dmg:abc", "update_available", "New update", "body")?;
        let mut stale = list(&paths)?.pop().expect("queued event");
        stale.status = DeliveryStatus::Sending;
        stale.next_attempt_at = None;
        write_event(&event_path(&paths, &stale.key), &stale)?;

        assert_eq!(deliver_due(&paths).await?, 1);
        let event = list(&paths)?.pop().expect("delivered event");
        assert_eq!(event.status, DeliveryStatus::Sent);
        let body = fs::read_to_string(delivered_mail)?;
        assert!(body.contains("To: test@example.invalid"));
        assert!(body.contains("Event key: dmg:abc"));
        Ok(())
    }

    #[tokio::test]
    async fn failed_transport_retries_without_recording_transport_output() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir)?;
        fs::write(
            &paths.config_file,
            "[email_notifications]\nenabled = true\nto = \"test@example.invalid\"\nsendmail_command = \"/usr/bin/false\"\nmax_attempts = 2\n",
        )?;
        enqueue(&paths, "dmg:def", "update_available", "New update", "body")?;
        assert_eq!(deliver_due(&paths).await?, 0);
        let event = list(&paths)?.pop().expect("failed event");
        assert_eq!(event.status, DeliveryStatus::Pending);
        assert_eq!(event.attempts, 1);
        assert_eq!(
            event.last_error.as_deref(),
            Some("sendmail delivery failed; inspect the local MTA log")
        );
        Ok(())
    }

    #[tokio::test]
    async fn timed_out_transport_is_bounded_and_retriable() -> Result<()> {
        let temp = tempdir()?;
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.config_dir)?;
        fs::write(
            &paths.config_file,
            "[email_notifications]\nenabled = true\nto = \"test@example.invalid\"\nsendmail_command = \"/bin/sh\"\nsendmail_args = [\"-c\", \"sleep 30\"]\ntimeout_seconds = 1\n",
        )?;
        enqueue(
            &paths,
            "dmg:timeout",
            "update_available",
            "New update",
            "body",
        )?;
        let started = std::time::Instant::now();
        assert_eq!(deliver_due(&paths).await?, 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let event = list(&paths)?.pop().expect("timed out event");
        assert_eq!(event.status, DeliveryStatus::Pending);
        assert_eq!(event.attempts, 1);
        Ok(())
    }
}
