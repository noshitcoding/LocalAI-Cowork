use super::*;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleRecurrence {
    Cron {
        expression: String,
    },
    Once {
        at_ms: i64,
    },
    Daily {
        hour: u32,
        minute: u32,
    },
    /// Monday = 1, Sunday = 7. Ambiguous local times use their first occurrence;
    /// nonexistent DST times are skipped, never shifted to another wall time.
    Weekly {
        weekday: u32,
        hour: u32,
        minute: u32,
    },
    Interval {
        seconds: u32,
    },
}
