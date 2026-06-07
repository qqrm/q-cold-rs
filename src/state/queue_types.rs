use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueRunStatus {
    Running,
    Waiting,
    Starting,
    Stopping,
    Stopped,
    Failed,
    Success,
    Unknown(String),
}

impl QueueRunStatus {
    pub fn from_db_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "starting" => Self::Starting,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "failed" => Self::Failed,
            "success" => Self::Success,
            _ => Self::Unknown(value),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Starting => "starting",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Success => "success",
            Self::Unknown(value) => value,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::Waiting | Self::Starting | Self::Stopping
        )
    }

    pub fn is_appendable(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::Waiting | Self::Starting | Self::Stopped
        )
    }

    pub fn is_editable(&self) -> bool {
        self.is_appendable()
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn is_failed(&self) -> bool {
        self.is_terminal() && matches!(self, Self::Failed)
    }
}

impl Serialize for QueueRunStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QueueRunStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_db_value)
    }
}

impl fmt::Display for QueueRunStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for QueueRunStatus {
    fn from(value: &str) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&String> for QueueRunStatus {
    fn from(value: &String) -> Self {
        Self::from_db_value(value.clone())
    }
}

impl From<String> for QueueRunStatus {
    fn from(value: String) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&QueueRunStatus> for QueueRunStatus {
    fn from(value: &QueueRunStatus) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for QueueRunStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<QueueRunStatus> for &str {
    fn eq(&self, other: &QueueRunStatus) -> bool {
        *self == other.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueItemStatus {
    Pending,
    Waiting,
    Starting,
    Running,
    Stopping,
    Stopped,
    Paused,
    Success,
    Failed,
    Blocked,
    Unknown(String),
}

impl QueueItemStatus {
    pub fn from_db_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "pending" => Self::Pending,
            "waiting" => Self::Waiting,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopping" => Self::Stopping,
            "stopped" => Self::Stopped,
            "paused" => Self::Paused,
            "success" => Self::Success,
            "failed" => Self::Failed,
            "blocked" => Self::Blocked,
            _ => Self::Unknown(value),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Waiting => "waiting",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Paused => "paused",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Unknown(value) => value,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Blocked)
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Waiting | Self::Stopping
        )
    }

    pub fn is_starting_or_running(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    pub fn has_executor_session(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    pub fn is_pending_or_waiting(&self) -> bool {
        matches!(self, Self::Pending | Self::Waiting)
    }

    pub fn is_stopped_or_paused(&self) -> bool {
        matches!(self, Self::Stopped | Self::Paused)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, Self::Paused)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn is_failed_or_blocked(&self) -> bool {
        matches!(self, Self::Failed | Self::Blocked)
    }
}

impl Serialize for QueueItemStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QueueItemStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_db_value)
    }
}

impl fmt::Display for QueueItemStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for QueueItemStatus {
    fn from(value: &str) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&String> for QueueItemStatus {
    fn from(value: &String) -> Self {
        Self::from_db_value(value.clone())
    }
}

impl From<String> for QueueItemStatus {
    fn from(value: String) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&QueueItemStatus> for QueueItemStatus {
    fn from(value: &QueueItemStatus) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for QueueItemStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<QueueItemStatus> for &str {
    fn eq(&self, other: &QueueItemStatus) -> bool {
        *self == other.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueExecutionMode {
    Sequence,
    Graph,
    Unknown(String),
}

impl QueueExecutionMode {
    pub fn from_db_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "sequence" => Self::Sequence,
            "graph" => Self::Graph,
            _ => Self::Unknown(value),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Sequence => "sequence",
            Self::Graph => "graph",
            Self::Unknown(value) => value,
        }
    }

    pub fn is_graph(&self) -> bool {
        matches!(self, Self::Graph)
    }
}

impl Serialize for QueueExecutionMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QueueExecutionMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_db_value)
    }
}

impl fmt::Display for QueueExecutionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for QueueExecutionMode {
    fn from(value: &str) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&String> for QueueExecutionMode {
    fn from(value: &String) -> Self {
        Self::from_db_value(value.clone())
    }
}

impl From<String> for QueueExecutionMode {
    fn from(value: String) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&QueueExecutionMode> for QueueExecutionMode {
    fn from(value: &QueueExecutionMode) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for QueueExecutionMode {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<QueueExecutionMode> for &str {
    fn eq(&self, other: &QueueExecutionMode) -> bool {
        *self == other.as_str()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueExecutionHost {
    Local,
    RemoteNative,
    Unknown(String),
}

impl QueueExecutionHost {
    pub fn from_db_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "local" => Self::Local,
            "remote-native" => Self::RemoteNative,
            _ => Self::Unknown(value),
        }
    }

    pub fn from_setting(value: &str) -> Self {
        match value.trim() {
            "local" => Self::Local,
            "remote" | "remote-native" | "remote_native" => Self::RemoteNative,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::RemoteNative => "remote-native",
            Self::Unknown(value) => value,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn is_remote_native(&self) -> bool {
        matches!(self, Self::RemoteNative)
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Self::Local | Self::RemoteNative)
    }
}

impl Serialize for QueueExecutionHost {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QueueExecutionHost {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_db_value)
    }
}

impl fmt::Display for QueueExecutionHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for QueueExecutionHost {
    fn from(value: &str) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&String> for QueueExecutionHost {
    fn from(value: &String) -> Self {
        Self::from_db_value(value.clone())
    }
}

impl From<String> for QueueExecutionHost {
    fn from(value: String) -> Self {
        Self::from_db_value(value)
    }
}

impl From<&QueueExecutionHost> for QueueExecutionHost {
    fn from(value: &QueueExecutionHost) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for QueueExecutionHost {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<QueueExecutionHost> for &str {
    fn eq(&self, other: &QueueExecutionHost) -> bool {
        *self == other.as_str()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueueTaskClass {
    Cheap,
    #[default]
    Mid,
    Heavy,
}

impl QueueTaskClass {
    pub fn from_db_value(value: impl AsRef<str>) -> Self {
        match value.as_ref().trim() {
            "cheap" => Self::Cheap,
            "heavy" => Self::Heavy,
            _ => Self::Mid,
        }
    }

    pub fn from_setting(value: Option<&str>) -> Self {
        value.map_or(Self::Mid, Self::from_db_value)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Mid => "mid",
            Self::Heavy => "heavy",
        }
    }
}

impl Serialize for QueueTaskClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QueueTaskClass {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_db_value)
    }
}

impl fmt::Display for QueueTaskClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for QueueTaskClass {
    fn from(value: &str) -> Self {
        Self::from_db_value(value)
    }
}

impl From<String> for QueueTaskClass {
    fn from(value: String) -> Self {
        Self::from_db_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{QueueExecutionHost, QueueExecutionMode, QueueItemStatus, QueueRunStatus};

    #[test]
    fn queue_statuses_serialize_as_existing_strings() {
        assert_eq!(
            serde_json::to_string(&QueueRunStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&QueueItemStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&QueueExecutionMode::Graph).unwrap(),
            "\"graph\""
        );
        assert_eq!(
            serde_json::to_string(&QueueExecutionHost::RemoteNative).unwrap(),
            "\"remote-native\""
        );
    }

    #[test]
    fn queue_statuses_deserialize_from_existing_strings() {
        assert_eq!(
            serde_json::from_str::<QueueRunStatus>("\"stopped\"").unwrap(),
            QueueRunStatus::Stopped
        );
        assert_eq!(
            serde_json::from_str::<QueueItemStatus>("\"blocked\"").unwrap(),
            QueueItemStatus::Blocked
        );
        assert_eq!(
            serde_json::from_str::<QueueExecutionMode>("\"sequence\"").unwrap(),
            QueueExecutionMode::Sequence
        );
        assert_eq!(
            serde_json::from_str::<QueueExecutionHost>("\"local\"").unwrap(),
            QueueExecutionHost::Local
        );
    }

    #[test]
    fn unknown_db_values_remain_explicit_and_non_terminal() {
        let run = QueueRunStatus::from_db_value("legacy-running-ish");
        let item = QueueItemStatus::from_db_value("legacy-success-ish");
        let mode = QueueExecutionMode::from_db_value("legacy-graph-ish");
        let host = QueueExecutionHost::from_db_value("legacy-remote-ish");

        assert_eq!(run.as_str(), "legacy-running-ish");
        assert_eq!(item.as_str(), "legacy-success-ish");
        assert_eq!(mode.as_str(), "legacy-graph-ish");
        assert_eq!(host.as_str(), "legacy-remote-ish");
        assert!(!run.is_terminal());
        assert!(!item.is_terminal());
        assert!(!mode.is_graph());
        assert!(!host.is_known());
        assert_eq!(
            serde_json::to_string(&item).unwrap(),
            "\"legacy-success-ish\""
        );
    }

    #[test]
    fn queue_status_classification_is_canonical() {
        assert!(QueueRunStatus::Running.is_active());
        assert!(QueueRunStatus::Stopped.is_appendable());
        assert!(QueueRunStatus::Success.is_terminal());
        assert!(QueueRunStatus::Failed.is_terminal());
        assert!(!QueueRunStatus::Stopped.is_terminal());

        assert!(QueueItemStatus::Pending.is_pending_or_waiting());
        assert!(QueueItemStatus::Waiting.is_active());
        assert!(QueueItemStatus::Starting.is_starting_or_running());
        assert!(QueueItemStatus::Stopping.has_executor_session());
        assert!(QueueItemStatus::Success.is_terminal());
        assert!(QueueItemStatus::Failed.is_failed_or_blocked());
        assert!(QueueItemStatus::Blocked.is_failed_or_blocked());
        assert!(!QueueItemStatus::Stopped.is_terminal());
        assert!(!QueueItemStatus::Paused.is_terminal());
    }
}
