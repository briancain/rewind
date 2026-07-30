//! Video-domain value types shared across services, so these values are type-checked rather than
//! passed around as bare strings. Both enums serialize to/from their lowercase string form (the
//! representation stored in the `videos` table and carried on the DynamoDB stream).
//!
//! NOTE: the DynamoDB stream wire form of these values (e.g. `{"S":"deleted"}`, `{"S":"public"}`)
//! also appears in infrastructure (the cascade-cleanup EventBridge Pipe filter). Keep those
//! literals in sync with the strings below.

use serde::{Deserialize, Serialize};

/// The lifecycle status of a video. `draft` is the create-time default; `processing`/`published`/
/// `failed` are set by the transcode pipeline; `deleted` is the soft-delete tombstone.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoStatus {
    #[default]
    Draft,
    Processing,
    Published,
    Failed,
    Deleted,
}

impl VideoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Processing => "processing",
            Self::Published => "published",
            Self::Failed => "failed",
            Self::Deleted => "deleted",
        }
    }
}

impl std::str::FromStr for VideoStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft" => Ok(Self::Draft),
            "processing" => Ok(Self::Processing),
            "published" => Ok(Self::Published),
            "failed" => Ok(Self::Failed),
            "deleted" => Ok(Self::Deleted),
            _ => Err(format!("invalid video status: {}", s)),
        }
    }
}

/// Who can discover/watch a video. `public` (default) is listed everywhere; `unlisted` is reachable
/// only by direct link; `private` is owner-only.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Public,
    Unlisted,
    Private,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Unlisted => "unlisted",
            Self::Private => "private",
        }
    }
}

impl std::str::FromStr for Visibility {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public" => Ok(Self::Public),
            "unlisted" => Ok(Self::Unlisted),
            "private" => Ok(Self::Private),
            _ => Err(format!("invalid visibility: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_status_as_str_matches_wire_values() {
        assert_eq!(VideoStatus::Draft.as_str(), "draft");
        assert_eq!(VideoStatus::Processing.as_str(), "processing");
        assert_eq!(VideoStatus::Published.as_str(), "published");
        assert_eq!(VideoStatus::Failed.as_str(), "failed");
        assert_eq!(VideoStatus::Deleted.as_str(), "deleted");
    }

    #[test]
    fn video_status_from_str_roundtrips_every_variant() {
        for s in ["draft", "processing", "published", "failed", "deleted"] {
            let parsed: VideoStatus = s.parse().unwrap();
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn video_status_from_str_rejects_unknown() {
        assert!("ready".parse::<VideoStatus>().is_err());
        assert!("".parse::<VideoStatus>().is_err());
    }

    #[test]
    fn video_status_default_is_draft() {
        assert_eq!(VideoStatus::default(), VideoStatus::Draft);
    }

    #[test]
    fn video_status_serde_uses_lowercase() {
        assert_eq!(
            serde_json::to_string(&VideoStatus::Published).unwrap(),
            "\"published\""
        );
        let parsed: VideoStatus = serde_json::from_str("\"deleted\"").unwrap();
        assert_eq!(parsed, VideoStatus::Deleted);
    }

    #[test]
    fn visibility_as_str_matches_wire_values() {
        assert_eq!(Visibility::Public.as_str(), "public");
        assert_eq!(Visibility::Unlisted.as_str(), "unlisted");
        assert_eq!(Visibility::Private.as_str(), "private");
    }

    #[test]
    fn visibility_from_str_roundtrips_every_variant() {
        for s in ["public", "unlisted", "private"] {
            let parsed: Visibility = s.parse().unwrap();
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn visibility_from_str_rejects_unknown() {
        assert!("secret".parse::<Visibility>().is_err());
    }

    #[test]
    fn visibility_default_is_public() {
        assert_eq!(Visibility::default(), Visibility::Public);
    }

    #[test]
    fn visibility_serde_uses_lowercase() {
        assert_eq!(
            serde_json::to_string(&Visibility::Private).unwrap(),
            "\"private\""
        );
        let parsed: Visibility = serde_json::from_str("\"unlisted\"").unwrap();
        assert_eq!(parsed, Visibility::Unlisted);
    }
}
