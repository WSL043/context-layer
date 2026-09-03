use context_contracts::{EventEnvelopeV2, SensitivityClass};
use context_platform_windows::{ForegroundActivity, foreground_activity, input_idle_millis};
use serde_json::json;
use time::OffsetDateTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputIdleBucket {
    UnderOneMinute,
    OneToFiveMinutes,
    FiveToThirtyMinutes,
    ThirtyMinutesOrMore,
}

impl InputIdleBucket {
    fn from_millis(idle_millis: u64) -> Self {
        match idle_millis {
            0..60_000 => Self::UnderOneMinute,
            60_000..300_000 => Self::OneToFiveMinutes,
            300_000..1_800_000 => Self::FiveToThirtyMinutes,
            _ => Self::ThirtyMinutesOrMore,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::UnderOneMinute => "lt_1m",
            Self::OneToFiveMinutes => "1m_to_5m",
            Self::FiveToThirtyMinutes => "5m_to_30m",
            Self::ThirtyMinutesOrMore => "gte_30m",
        }
    }
}

#[derive(Default)]
pub struct PersonalActivitySampler {
    last_foreground: Option<Option<ForegroundActivity>>,
    last_idle_bucket: Option<InputIdleBucket>,
    device_id: Option<String>,
    foreground_error: Option<String>,
    idle_error: Option<String>,
}

#[derive(Default)]
pub struct PersonalPoll {
    pub events: Vec<EventEnvelopeV2>,
    pub diagnostics: Vec<String>,
}

impl PersonalActivitySampler {
    pub fn new() -> Self {
        Self {
            device_id: std::env::var("COMPUTERNAME").ok(),
            ..Self::default()
        }
    }

    pub fn poll(&mut self, observed_at: OffsetDateTime) -> PersonalPoll {
        let mut poll = PersonalPoll::default();

        match foreground_activity() {
            Ok(foreground) => {
                if self.foreground_error.take().is_some() {
                    poll.diagnostics
                        .push("windows foreground activity sampling recovered".into());
                }
                if let Some(event) = self.observe_foreground(foreground, observed_at) {
                    poll.events.push(event);
                }
            }
            Err(error) => {
                let message = error.to_string();
                if self.foreground_error.as_deref() != Some(message.as_str()) {
                    poll.diagnostics
                        .push(format!("windows foreground activity sampling failed: {message}"));
                    self.foreground_error = Some(message);
                }
            }
        }

        match input_idle_millis() {
            Ok(idle_millis) => {
                if self.idle_error.take().is_some() {
                    poll.diagnostics
                        .push("windows input-idle sampling recovered".into());
                }
                if let Some(event) = self.observe_idle(idle_millis, observed_at) {
                    poll.events.push(event);
                }
            }
            Err(error) => {
                let message = error.to_string();
                if self.idle_error.as_deref() != Some(message.as_str()) {
                    poll.diagnostics
                        .push(format!("windows input-idle sampling failed: {message}"));
                    self.idle_error = Some(message);
                }
            }
        }

        poll
    }

    fn observe_foreground(
        &mut self,
        foreground: Option<ForegroundActivity>,
        observed_at: OffsetDateTime,
    ) -> Option<EventEnvelopeV2> {
        if self.last_foreground.as_ref() == Some(&foreground) {
            return None;
        }
        self.last_foreground = Some(foreground.clone());

        let payload = match foreground {
            Some(activity) => json!({
                "process_id": activity.process_id,
                "process_path": activity.process_path,
                "window_title": activity.window_title,
            }),
            None => json!({
                "process_id": null,
                "process_path": null,
                "window_title": null,
            }),
        };
        let mut event = EventEnvelopeV2::observed(
            "ui.foreground_changed",
            "windows.foreground",
            "scope.personal",
            observed_at,
            payload,
            "windows-foreground-v1",
            "GetForegroundWindow + window title + process image",
        );
        event.device_id = self.device_id.clone();
        event.sensitivity = SensitivityClass::Sensitive;
        Some(event)
    }

    fn observe_idle(
        &mut self,
        idle_millis: u64,
        observed_at: OffsetDateTime,
    ) -> Option<EventEnvelopeV2> {
        let bucket = InputIdleBucket::from_millis(idle_millis);
        if self.last_idle_bucket == Some(bucket) {
            return None;
        }
        self.last_idle_bucket = Some(bucket);

        let mut event = EventEnvelopeV2::observed(
            "user.input_idle_bucket_changed",
            "windows.input-idle",
            "scope.personal",
            observed_at,
            json!({
                "idle_millis": idle_millis,
                "bucket": bucket.label(),
                "measurement": "last_input_tick",
            }),
            "windows-input-idle-v1",
            "GetLastInputInfo sampled into sparse duration buckets",
        );
        event.device_id = self.device_id.clone();
        event.sensitivity = SensitivityClass::Metadata;
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(second: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(second).unwrap()
    }

    #[test]
    fn idle_buckets_are_sparse_and_boundary_stable() {
        assert_eq!(InputIdleBucket::from_millis(0).label(), "lt_1m");
        assert_eq!(InputIdleBucket::from_millis(59_999).label(), "lt_1m");
        assert_eq!(InputIdleBucket::from_millis(60_000).label(), "1m_to_5m");
        assert_eq!(InputIdleBucket::from_millis(299_999).label(), "1m_to_5m");
        assert_eq!(InputIdleBucket::from_millis(300_000).label(), "5m_to_30m");
        assert_eq!(InputIdleBucket::from_millis(1_800_000).label(), "gte_30m");
    }

    #[test]
    fn duplicate_foreground_snapshots_do_not_emit_duplicate_events() {
        let mut sampler = PersonalActivitySampler::default();
        let activity = ForegroundActivity {
            process_id: 42,
            process_path: Some(r"C:\Example\example.exe".into()),
            window_title: "Same document".into(),
        };

        let first = sampler.observe_foreground(Some(activity.clone()), at(1)).unwrap();
        assert_eq!(first.event_type, "ui.foreground_changed");
        assert!(
            sampler
                .observe_foreground(Some(activity.clone()), at(2))
                .is_none()
        );

        let mut changed = activity;
        changed.window_title = "Different document".into();
        assert!(
            sampler
                .observe_foreground(Some(changed), at(3))
                .is_some()
        );
    }

    #[test]
    fn input_idle_emits_only_when_bucket_changes() {
        let mut sampler = PersonalActivitySampler::default();
        let first = sampler.observe_idle(1_000, at(1)).unwrap();
        assert_eq!(first.payload["bucket"], "lt_1m");
        assert!(sampler.observe_idle(30_000, at(2)).is_none());
        let changed = sampler.observe_idle(60_000, at(3)).unwrap();
        assert_eq!(changed.payload["bucket"], "1m_to_5m");
    }
}
