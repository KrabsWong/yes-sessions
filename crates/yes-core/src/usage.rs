//! Independently cached, message-level token statistics. Detail subtree totals are never used.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

use crate::{
    AppType, MessageType, ProviderRegistry, Session, SessionDetail,
    model::{SessionKind, TokenUsage},
};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRecord {
    pub date: Option<NaiveDate>,
    pub app_type: AppType,
    pub project: Option<String>,
    pub model: Option<String>,
    pub session_id: String,
    pub session_title: String,
    pub kind: SessionKind,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageDataset {
    pub records: Vec<UsageRecord>,
    pub failed_sessions: usize,
    pub failed_providers: usize,
}

#[derive(Clone, Debug, Default)]
pub struct UsageFilter {
    pub from: Option<NaiveDate>,
    /// Inclusive final local calendar day.
    pub until: Option<NaiveDate>,
    pub app_type: Option<AppType>,
    /// Empty string selects records with an unknown project/model.
    pub project: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsageDimension {
    #[default]
    Tool,
    Project,
    Model,
    Session,
    Kind,
}

#[derive(Clone, Debug, Default)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub records: usize,
    pub usage_records: usize,
    pub input_records: usize,
    pub output_records: usize,
    pub total_records: usize,
    pub cache_records: usize,
    pub sessions: usize,
    /// Records with a valid input/cache pair; cache rate uses only these pairs.
    pub cache_rate_records: usize,
    cache_input: u64,
    cache_read: u64,
}

impl UsageTotals {
    pub fn incomplete(&self) -> bool {
        self.input_records < self.records
            || self.output_records < self.records
            || self.total_records < self.records
            || self.cache_records < self.records
    }
    pub fn cache_hit_rate(&self) -> Option<f64> {
        (self.cache_input > 0).then(|| self.cache_read as f64 / self.cache_input as f64 * 100.0)
    }
    fn add(&mut self, record: &UsageRecord) {
        self.records += 1;
        let Some(usage) = &record.usage else {
            return;
        };
        self.usage_records += 1;
        for (value, sum, count) in [
            (
                usage.input_tokens,
                &mut self.input_tokens,
                &mut self.input_records,
            ),
            (
                usage.output_tokens,
                &mut self.output_tokens,
                &mut self.output_records,
            ),
            (
                usage.total_tokens,
                &mut self.total_tokens,
                &mut self.total_records,
            ),
            (
                usage.cache_read_tokens,
                &mut self.cache_read_tokens,
                &mut self.cache_records,
            ),
        ] {
            if let Some(value) = value {
                *sum = sum.saturating_add(value);
                *count += 1;
            }
        }
        if let (Some(input), Some(cached)) = (usage.input_tokens, usage.cache_read_tokens) {
            if cached <= input {
                self.cache_input = self.cache_input.saturating_add(input);
                self.cache_read = self.cache_read.saturating_add(cached);
                self.cache_rate_records += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct UsageGroup {
    pub key: String,
    pub label: String,
    pub app_type: Option<AppType>,
    pub session_id: Option<String>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Default)]
pub struct UsageReport {
    pub totals: UsageTotals,
    pub daily: Vec<(NaiveDate, UsageTotals)>,
    pub groups: Vec<UsageGroup>,
    /// Records matching the non-date filters whose timestamps cannot be parsed.
    pub unknown_date_records: usize,
}

#[derive(Default)]
struct Accumulator {
    totals: UsageTotals,
    sessions: HashSet<(AppType, String)>,
}
impl Accumulator {
    fn add(&mut self, record: &UsageRecord) {
        self.totals.add(record);
        self.sessions
            .insert((record.app_type, record.session_id.clone()));
        self.totals.sessions = self.sessions.len();
    }
}

impl UsageDataset {
    pub fn projects(&self) -> Vec<String> {
        self.options(|record| record.project.as_deref())
    }
    pub fn models(&self) -> Vec<(AppType, String)> {
        let mut models: Vec<_> = self
            .records
            .iter()
            .map(|record| (record.app_type, record.model.clone().unwrap_or_default()))
            .collect();
        models.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));
        models.dedup();
        models
    }
    fn options(&self, field: impl Fn(&UsageRecord) -> Option<&str>) -> Vec<String> {
        let mut values: Vec<_> = self
            .records
            .iter()
            .map(|record| field(record).unwrap_or_default().to_owned())
            .collect();
        values.sort();
        values.dedup();
        values
    }
    pub fn aggregate(&self, filter: &UsageFilter, dimension: UsageDimension) -> UsageReport {
        let mut all = Accumulator::default();
        let mut daily: BTreeMap<NaiveDate, Accumulator> = BTreeMap::new();
        let mut groups: HashMap<String, (UsageGroup, Accumulator)> = HashMap::new();
        let mut unknown_date_records = 0;
        for record in &self.records {
            if filter.app_type.is_some_and(|app| app != record.app_type)
                || filter
                    .project
                    .as_deref()
                    .is_some_and(|value| value != record.project.as_deref().unwrap_or_default())
                || filter
                    .model
                    .as_deref()
                    .is_some_and(|value| value != record.model.as_deref().unwrap_or_default())
            {
                continue;
            }
            if record.date.is_none() {
                unknown_date_records += 1;
            }
            if (filter.from.is_some() || filter.until.is_some()) && record.date.is_none()
                || record.date.is_some_and(|date| {
                    filter.from.is_some_and(|from| date < from)
                        || filter.until.is_some_and(|until| date > until)
                })
            {
                continue;
            }
            all.add(record);
            if let Some(date) = record.date {
                daily.entry(date).or_default().add(record);
            }
            let key = match dimension {
                UsageDimension::Tool => record.app_type.as_str().to_owned(),
                UsageDimension::Project => record.project.clone().unwrap_or_default(),
                UsageDimension::Model => record.model.clone().unwrap_or_default(),
                UsageDimension::Session => format!("{}:{}", record.app_type, record.session_id),
                UsageDimension::Kind => match record.kind {
                    SessionKind::Main => "main",
                    SessionKind::Subagent => "subagent",
                }
                .to_owned(),
            };
            let label = match dimension {
                UsageDimension::Tool => record.app_type.display_name().to_owned(),
                UsageDimension::Session => record.session_title.clone(),
                _ => key.clone(),
            };
            groups
                .entry(key.clone())
                .or_insert_with(|| {
                    (
                        UsageGroup {
                            key,
                            label,
                            app_type: matches!(
                                dimension,
                                UsageDimension::Tool | UsageDimension::Session
                            )
                            .then_some(record.app_type),
                            session_id: (dimension == UsageDimension::Session)
                                .then(|| record.session_id.clone()),
                            totals: UsageTotals::default(),
                        },
                        Accumulator::default(),
                    )
                })
                .1
                .add(record);
        }
        let mut groups: Vec<_> = groups
            .into_values()
            .map(|(mut group, accumulator)| {
                group.totals = accumulator.totals;
                group
            })
            .collect();
        groups.sort_by(|a, b| {
            b.totals
                .total_tokens
                .cmp(&a.totals.total_tokens)
                .then_with(|| a.key.cmp(&b.key))
        });
        UsageReport {
            totals: all.totals,
            daily: daily
                .into_iter()
                .map(|(date, accumulator)| (date, accumulator.totals))
                .collect(),
            groups,
            unknown_date_records,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    path: PathBuf,
    source: (u64, SystemTime),
    wal: Option<(u64, SystemTime)>,
    updated: i64,
}
fn metadata(path: &Path) -> Option<(u64, SystemTime)> {
    let metadata = path.metadata().ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}
fn fingerprint(session: &Session) -> Option<Fingerprint> {
    let wal = if session.app_type == AppType::OpenCode {
        let mut path = session.file_path.as_os_str().to_owned();
        path.push("-wal");
        metadata(Path::new(&path))
    } else {
        None
    };
    Some(Fingerprint {
        path: session.file_path.clone(),
        source: metadata(&session.file_path)?,
        wal,
        updated: session.updated_at,
    })
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedSession {
    fingerprint: Option<Fingerprint>,
    session: SessionIdentity,
    records: Vec<UsageRecord>,
}
pub struct UsageCache {
    sessions: HashMap<(AppType, String), CachedSession>,
    timezone: String,
    failed_sessions: usize,
    failed_providers: usize,
}

impl Default for UsageCache {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            timezone: timezone_marker(),
            failed_sessions: 0,
            failed_providers: 0,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SessionIdentity {
    app_type: AppType,
    id: String,
    project: Option<PathBuf>,
    kind: SessionKind,
    title: String,
}
impl From<&Session> for SessionIdentity {
    fn from(session: &Session) -> Self {
        Self {
            app_type: session.app_type,
            id: session.id.clone(),
            project: session.directory.clone(),
            kind: session.kind,
            title: session_title(session),
        }
    }
}
fn session_title(session: &Session) -> String {
    let title = if session.first_message.is_empty() {
        &session.file_name
    } else {
        &session.first_message
    };
    title.chars().take(120).collect()
}

#[derive(Serialize, Deserialize)]
struct DiskCache {
    version: u32,
    timezone: String,
    sessions: Vec<CachedSession>,
    failed_sessions: usize,
    failed_providers: usize,
}
fn timezone_marker() -> String {
    format!(
        "{:?}|{:?}|{}",
        std::env::var_os("TZ"),
        fs::read_link("/etc/localtime").ok(),
        Local::now().offset()
    )
}

impl UsageCache {
    pub fn default_path() -> Option<PathBuf> {
        dirs::cache_dir().map(|path| path.join("yes-sessions/token-usage-v1.json"))
    }

    /// Invalid or obsolete caches are disposable and rebuilt from source files.
    pub fn load(path: &Path) -> Option<Self> {
        let disk: DiskCache =
            serde_json::from_reader(BufReader::new(fs::File::open(path).ok()?)).ok()?;
        if disk.version != 1 || disk.timezone != timezone_marker() {
            return None;
        }
        Some(Self {
            sessions: disk
                .sessions
                .into_iter()
                .map(|entry| ((entry.session.app_type, entry.session.id.clone()), entry))
                .collect(),
            timezone: disk.timezone,
            failed_sessions: disk.failed_sessions,
            failed_providers: disk.failed_providers,
        })
    }

    /// Replace the previous snapshot atomically; a failed write leaves it intact.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let (temporary, file) = loop {
            let temporary = parent.join(format!(
                ".token-usage-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            let disk = DiskCache {
                version: 1,
                timezone: self.timezone.clone(),
                sessions: self.sessions.values().cloned().collect(),
                failed_sessions: self.failed_sessions,
                failed_providers: self.failed_providers,
            };
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &disk)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Last completed statistics, without reading provider files.
    pub fn snapshot(&self) -> UsageDataset {
        let mut dataset = UsageDataset {
            failed_sessions: self.failed_sessions,
            failed_providers: self.failed_providers,
            ..Default::default()
        };
        let mut entries: Vec<_> = self.sessions.values().collect();
        entries.sort_by(|a, b| {
            a.session
                .app_type
                .as_str()
                .cmp(b.session.app_type.as_str())
                .then_with(|| a.session.id.cmp(&b.session.id))
        });
        let mut emitted = HashSet::new();
        for entry in entries {
            let canonical = entry
                .records
                .first()
                .map(|record| record.session_id.as_str())
                .unwrap_or(&entry.session.id);
            if emitted.insert((entry.session.app_type, canonical)) {
                dataset.records.extend(entry.records.iter().cloned());
            }
        }
        dataset
    }

    /// Intended for a background worker. Unchanged files are not parsed again. Failures are
    /// excluded from this snapshot and retried next refresh, never silently replaced by zero.
    pub fn refresh(&mut self, registry: &ProviderRegistry) -> UsageDataset {
        let timezone = timezone_marker();
        if self.timezone != timezone {
            self.sessions.clear();
            self.timezone = timezone;
        }
        let mut dataset = UsageDataset::default();
        let mut retained = HashSet::new();
        for app in AppType::ALL {
            let Some(provider) = registry.get(app).filter(|provider| provider.is_available())
            else {
                continue;
            };
            let listed = match provider.sessions() {
                Ok(sessions) => sessions,
                Err(_) => {
                    dataset.failed_providers += 1;
                    continue;
                }
            };
            for session in listed {
                let key = (app, session.id.clone());
                if !retained.insert(key.clone()) {
                    continue;
                }
                let stamp = fingerprint(&session);
                let cached = self.sessions.get(&key).filter(|cached| {
                    stamp.is_some()
                        && cached.fingerprint == stamp
                        && cached.session == SessionIdentity::from(&session)
                });
                let entry = if let Some(cached) = cached {
                    cached.clone()
                } else {
                    match provider.session_detail_for_search(&session) {
                        Ok(Some(detail)) => CachedSession {
                            fingerprint: stamp,
                            session: SessionIdentity::from(&session),
                            records: records(&detail),
                        },
                        _ => {
                            dataset.failed_sessions += 1;
                            self.sessions.remove(&key);
                            continue;
                        }
                    }
                };
                self.sessions.insert(key, entry);
            }
        }
        self.sessions.retain(|key, _| retained.contains(key));
        self.failed_sessions = dataset.failed_sessions;
        self.failed_providers = dataset.failed_providers;
        self.snapshot()
    }
}

// Display messages may combine several requests. Preserve their original attribution
// separately so model switches, midnight crossings and absent dates remain accurate.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct UsageEvent {
    pub timestamp: Option<String>,
    pub model: Option<String>,
    pub usage: TokenUsage,
}

impl UsageEvent {
    pub fn attach(&self, message: &mut crate::SessionMessage) {
        message
            .metadata
            .entry("usage_events")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .expect("usage_events is an internal array")
            .push(serde_json::json!(self));
    }
}

pub(crate) fn apply_usage_events(message: &mut crate::SessionMessage, events: &[UsageEvent]) {
    message.usage = TokenUsage::aggregate(events.iter().map(|event| &event.usage));
    message
        .metadata
        .insert("usage_events".into(), serde_json::json!(events));
}

pub(crate) fn records(detail: &SessionDetail) -> Vec<UsageRecord> {
    detail
        .messages
        .iter()
        .filter(|message| message.usage.is_some() || message.message_type == MessageType::Assistant)
        .flat_map(|message| {
            let events = message
                .metadata
                .get("usage_events")
                .and_then(|events| serde_json::from_value::<Vec<UsageEvent>>(events.clone()).ok());
            let contributions = if let Some(events) = events {
                events
                    .into_iter()
                    .map(|event| (event.timestamp, event.model, Some(event.usage)))
                    .collect::<Vec<_>>()
            } else {
                vec![(
                    Some(message.timestamp.clone()),
                    message.model.clone(),
                    message.usage.clone(),
                )]
            };
            contributions
                .into_iter()
                .map(|(timestamp, model, usage)| UsageRecord {
                    date: timestamp
                        .as_deref()
                        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                        .map(|date| date.with_timezone(&Local).date_naive()),
                    app_type: detail.session.app_type,
                    project: detail
                        .session
                        .directory
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    model: model.filter(|model| !model.is_empty()),
                    session_id: detail.session.id.clone(),
                    session_title: session_title(&detail.session),
                    kind: detail.session.kind,
                    usage,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionMessage, SessionProvider};
    use std::sync::{Arc, Mutex};

    fn record(session: &str, day: Option<&str>, input: u64, cached: Option<u64>) -> UsageRecord {
        UsageRecord {
            date: day.map(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap()),
            app_type: AppType::Codex,
            project: Some("/work/a".into()),
            model: Some("model-a".into()),
            session_id: session.into(),
            session_title: session.into(),
            kind: SessionKind::Main,
            usage: Some(TokenUsage {
                input_tokens: Some(input),
                output_tokens: Some(10),
                total_tokens: Some(input + 10),
                cache_read_tokens: cached,
            }),
        }
    }

    #[test]
    fn expands_contributions_using_local_event_dates_and_preserves_unknowns() {
        use chrono::TimeZone;
        let mut detail = detail(Path::new("unused"), "session", SessionKind::Main);
        let first = Local
            .with_ymd_and_hms(2026, 9, 10, 23, 59, 59)
            .single()
            .unwrap();
        let second = Local
            .with_ymd_and_hms(2026, 9, 11, 0, 0, 1)
            .single()
            .unwrap();
        let usage = detail.messages[0].usage.clone().unwrap();
        apply_usage_events(
            &mut detail.messages[0],
            &[
                UsageEvent {
                    timestamp: Some(first.to_rfc3339()),
                    model: Some("first".into()),
                    usage: usage.clone(),
                },
                UsageEvent {
                    timestamp: Some(second.to_rfc3339()),
                    model: Some("second".into()),
                    usage: usage.clone(),
                },
                UsageEvent {
                    timestamp: None,
                    model: None,
                    usage,
                },
            ],
        );
        let dataset = UsageDataset {
            records: records(&detail),
            ..Default::default()
        };
        let report = dataset.aggregate(&UsageFilter::default(), UsageDimension::Model);
        assert_eq!(report.totals.total_tokens, 330);
        assert_eq!(report.unknown_date_records, 1);
        assert_eq!(report.groups.len(), 3);
        assert_eq!(report.daily[0].0, first.date_naive());
        assert_eq!(report.daily[1].0, second.date_naive());
        assert_eq!(report.daily[0].1.total_tokens, 110);
        assert_eq!(report.daily[1].1.total_tokens, 110);
    }

    #[test]
    fn filters_inclusive_dates_and_intersects_dimensions_without_losing_unknowns() {
        let first = record("one", Some("2026-09-10"), 100, Some(50));
        let mut second = record("two", Some("2026-09-11"), 200, None);
        second.model = None;
        let mut third = record("three", Some("2026-09-12"), 300, Some(0));
        third.project = Some("/work/b".into());
        let dataset = UsageDataset {
            records: vec![first, second, third, record("unknown", None, 999, None)],
            ..Default::default()
        };
        let filter = UsageFilter {
            from: Some(NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()),
            until: Some(NaiveDate::from_ymd_opt(2026, 9, 11).unwrap()),
            project: Some("/work/a".into()),
            ..Default::default()
        };
        let report = dataset.aggregate(&filter, UsageDimension::Model);
        assert_eq!(report.totals.total_tokens, 320);
        assert_eq!(report.totals.sessions, 2);
        assert_eq!(report.daily.len(), 2);
        assert_eq!(report.unknown_date_records, 1);
        assert!(report.groups.iter().any(|group| group.key.is_empty()));
        let report = dataset.aggregate(
            &UsageFilter {
                model: Some(String::new()),
                ..filter
            },
            UsageDimension::Session,
        );
        assert_eq!(report.totals.total_tokens, 210);
        assert_eq!(report.groups[0].session_id.as_deref(), Some("two"));
        assert_eq!(
            dataset
                .aggregate(&UsageFilter::default(), UsageDimension::Tool)
                .totals
                .total_tokens,
            1639
        );
    }

    #[test]
    fn model_options_keep_same_named_and_unknown_models_separate_by_agent() {
        let first = record("one", None, 100, Some(0));
        let mut second = record("two", None, 200, Some(0));
        second.app_type = AppType::Claude;
        let mut unknown_first = first.clone();
        unknown_first.model = None;
        let mut unknown_second = second.clone();
        unknown_second.model = None;
        let model = first.model.clone().unwrap();
        let dataset = UsageDataset {
            records: vec![first.clone(), second, unknown_first, unknown_second],
            ..Default::default()
        };
        let models = dataset.models();
        assert_eq!(models.len(), 4);
        assert!(models.contains(&(first.app_type, model.clone())));
        assert!(models.contains(&(AppType::Claude, model.clone())));
        assert!(models.contains(&(first.app_type, String::new())));
        assert!(models.contains(&(AppType::Claude, String::new())));
        for (app, expected) in [(first.app_type, 110), (AppType::Claude, 210)] {
            let report = dataset.aggregate(
                &UsageFilter {
                    app_type: Some(app),
                    model: Some(model.clone()),
                    ..Default::default()
                },
                UsageDimension::Tool,
            );
            assert_eq!(report.totals.total_tokens, expected);
            assert_eq!(report.groups.len(), 1);
            assert_eq!(report.groups[0].app_type, Some(app));
        }
        let mut repeated = dataset.clone();
        repeated.records.push(first);
        assert_eq!(repeated.models(), models);
    }

    #[test]
    fn known_sums_and_weighted_cache_rate_expose_missing_coverage() {
        let first = record("one", None, 100, Some(100));
        let second = record("one", None, 900, Some(0));
        let mut missing = record("two", None, 4000, None);
        missing.usage.as_mut().unwrap().total_tokens = None;
        let mut invalid = record("two", None, 100, Some(200));
        invalid.kind = SessionKind::Subagent;
        let mut absent = first.clone();
        absent.usage = None;
        let dataset = UsageDataset {
            records: vec![first, second, missing, invalid, absent],
            ..Default::default()
        };
        let report = dataset.aggregate(&UsageFilter::default(), UsageDimension::Kind);
        assert_eq!(report.totals.cache_hit_rate(), Some(10.0));
        assert_eq!(report.totals.cache_rate_records, 2);
        assert_eq!(report.totals.total_tokens, 1130);
        assert_eq!(report.totals.records, 5);
        assert_eq!(report.totals.usage_records, 4);
        assert_eq!(report.totals.total_records, 3);
        assert_eq!(report.totals.sessions, 2);
        assert!(report.totals.incomplete());
        assert_eq!(report.groups.len(), 2);
    }

    struct Fixture {
        details: Mutex<Vec<SessionDetail>>,
        reads: Mutex<usize>,
        app: AppType,
    }
    impl SessionProvider for Fixture {
        fn app_type(&self) -> AppType {
            self.app
        }
        fn is_available(&self) -> bool {
            true
        }
        fn sessions(&self) -> anyhow::Result<Vec<Session>> {
            Ok(self
                .details
                .lock()
                .unwrap()
                .iter()
                .map(|detail| detail.session.clone())
                .collect())
        }
        fn session_detail(&self, id: &str) -> anyhow::Result<Option<SessionDetail>> {
            *self.reads.lock().unwrap() += 1;
            Ok(self
                .details
                .lock()
                .unwrap()
                .iter()
                .find(|detail| detail.session.id == id)
                .cloned())
        }
    }
    struct Unavailable(AppType);
    impl SessionProvider for Unavailable {
        fn app_type(&self) -> AppType {
            self.0
        }
        fn is_available(&self) -> bool {
            false
        }
        fn sessions(&self) -> anyhow::Result<Vec<Session>> {
            unreachable!()
        }
        fn session_detail(&self, _: &str) -> anyhow::Result<Option<SessionDetail>> {
            unreachable!()
        }
    }
    fn detail(path: &Path, id: &str, kind: SessionKind) -> SessionDetail {
        let mut message =
            SessionMessage::text(MessageType::Assistant, "2026-09-10T12:00:00Z", "answer");
        message.usage = record(id, None, 100, Some(50)).usage;
        SessionDetail {
            subtree_usage: Some(TokenUsage {
                total_tokens: Some(9999),
                ..Default::default()
            }),
            session: Session {
                id: id.into(),
                app_type: AppType::OpenCode,
                file_name: id.into(),
                file_path: path.into(),
                created_at: 0,
                updated_at: 0,
                message_count: 1,
                first_message: id.into(),
                last_message: String::new(),
                directory: None,
                uuid: None,
                kind,
                parent_session_id: None,
                agent_type: None,
            },
            messages: vec![message],
        }
    }
    #[test]
    fn caches_unchanged_sessions_invalidates_wal_and_counts_children_once() {
        let directory = std::env::temp_dir().join(format!("yes-usage-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("db");
        std::fs::write(&path, "db").unwrap();
        let parent = detail(&path, "parent", SessionKind::Main);
        let provider = Arc::new(Fixture {
            details: Mutex::new(vec![
                parent.clone(),
                parent,
                detail(&path, "child", SessionKind::Subagent),
            ]),
            reads: Mutex::new(0),
            app: AppType::OpenCode,
        });
        let mut registry = ProviderRegistry::default();
        for app in AppType::ALL {
            registry.register(Arc::new(Unavailable(app)));
        }
        registry.register(provider.clone());
        let mut cache = UsageCache::default();
        let first = cache.refresh(&registry);
        assert_eq!(first.records.len(), 2);
        assert_eq!(
            first
                .aggregate(&UsageFilter::default(), UsageDimension::Kind)
                .totals
                .total_tokens,
            220
        );
        assert_eq!(*provider.reads.lock().unwrap(), 2);
        let cache_path = directory.join("cache.json");
        cache.save(&cache_path).unwrap();
        let mut cache = UsageCache::load(&cache_path).unwrap();
        assert_eq!(cache.snapshot().records.len(), 2);
        cache.refresh(&registry);
        assert_eq!(*provider.reads.lock().unwrap(), 2);
        std::fs::write(directory.join("db-wal"), "new transaction").unwrap();
        cache.refresh(&registry);
        assert_eq!(*provider.reads.lock().unwrap(), 4);
        cache.save(&cache_path).unwrap();
        let mut cache = UsageCache::load(&cache_path).unwrap();
        provider
            .details
            .lock()
            .unwrap()
            .retain(|detail| detail.session.id != "child");
        assert_eq!(cache.refresh(&registry).records.len(), 1);
        cache.save(&cache_path).unwrap();
        let mut cache = UsageCache::load(&cache_path).unwrap();
        std::fs::write(&path, "changed db").unwrap();
        cache.refresh(&registry);
        assert_eq!(*provider.reads.lock().unwrap(), 5);
        cache.timezone = "old timezone".into();
        cache.save(&cache_path).unwrap();
        assert!(UsageCache::load(&cache_path).is_none());
        cache.refresh(&registry);
        assert_eq!(*provider.reads.lock().unwrap(), 6);
        cache.save(&cache_path).unwrap();
        assert!(UsageCache::load(&cache_path).is_some());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn disk_cache_is_private_minimal_and_recovers_from_incompatible_files() {
        let directory = std::env::temp_dir().join(format!("yes-usage-disk-{}", std::process::id()));
        let path = directory.join("cache.json");
        assert!(UsageCache::load(&path).is_none());
        let mut detail = detail(Path::new("unused"), "one", SessionKind::Main);
        detail.session.first_message = format!("{}PRIVATE_FIRST_MESSAGE_TAIL", "a".repeat(120));
        detail.session.last_message = "PRIVATE_LAST_MESSAGE".into();
        detail.messages[0].content = Some("PRIVATE_ASSISTANT_CONTENT".into());
        let identity = SessionIdentity::from(&detail.session);
        let mut cache = UsageCache {
            failed_sessions: 2,
            failed_providers: 1,
            ..Default::default()
        };
        cache.sessions.insert(
            (identity.app_type, identity.id.clone()),
            CachedSession {
                fingerprint: None,
                session: identity,
                records: records(&detail),
            },
        );
        cache.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let json = fs::read_to_string(&path).unwrap();
        assert!(!json.contains("PRIVATE_"));
        assert!(!json.contains("last_message"));
        let loaded = UsageCache::load(&path).unwrap().snapshot();
        assert_eq!(loaded.failed_sessions, 2);
        assert_eq!(loaded.failed_providers, 1);
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].session_title.chars().count(), 120);
        assert_eq!(loaded.records[0].date, cache.snapshot().records[0].date);
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["version"] = 99.into();
        fs::write(&path, value.to_string()).unwrap();
        assert!(UsageCache::load(&path).is_none());
        value["version"] = 1.into();
        value["timezone"] = "different timezone".into();
        fs::write(&path, value.to_string()).unwrap();
        assert!(UsageCache::load(&path).is_none());
        fs::write(&path, "{broken").unwrap();
        assert!(UsageCache::load(&path).is_none());
        cache.save(&path).unwrap();
        assert!(UsageCache::load(&path).is_some());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn failed_reads_are_persisted_and_retried_after_restart() {
        struct Failing {
            listed: Mutex<bool>,
        }
        impl SessionProvider for Failing {
            fn app_type(&self) -> AppType {
                AppType::OpenCode
            }
            fn is_available(&self) -> bool {
                true
            }
            fn sessions(&self) -> anyhow::Result<Vec<Session>> {
                if *self.listed.lock().unwrap() {
                    Ok(vec![
                        detail(Path::new("missing"), "failed", SessionKind::Main).session,
                    ])
                } else {
                    anyhow::bail!("listing unavailable")
                }
            }
            fn session_detail(&self, _: &str) -> anyhow::Result<Option<SessionDetail>> {
                anyhow::bail!("detail unavailable")
            }
        }
        let directory =
            std::env::temp_dir().join(format!("yes-usage-failures-{}", std::process::id()));
        let path = directory.join("cache.json");
        let provider = Arc::new(Failing {
            listed: Mutex::new(true),
        });
        let mut registry = ProviderRegistry::default();
        for app in AppType::ALL {
            registry.register(Arc::new(Unavailable(app)));
        }
        registry.register(provider.clone());
        let mut cache = UsageCache::default();
        assert_eq!(cache.refresh(&registry).failed_sessions, 1);
        cache.save(&path).unwrap();
        let mut cache = UsageCache::load(&path).unwrap();
        assert_eq!(cache.snapshot().failed_sessions, 1);
        assert_eq!(cache.refresh(&registry).failed_sessions, 1);
        *provider.listed.lock().unwrap() = false;
        let dataset = cache.refresh(&registry);
        assert_eq!(dataset.failed_providers, 1);
        assert_eq!(dataset.failed_sessions, 0);
        cache.save(&path).unwrap();
        let dataset = UsageCache::load(&path).unwrap().snapshot();
        assert_eq!(dataset.failed_providers, 1);
        assert_eq!(dataset.failed_sessions, 0);
        assert!(dataset.records.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }
}
