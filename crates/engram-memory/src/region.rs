//! Brain regions. Memory is not one undifferentiated pool — it is partitioned the
//! way a brain is, and recall consults the regions that fit the *kind* of task at
//! hand (recall by experience type).

use serde::{Deserialize, Serialize};

/// Where a memory lives. Recall is region-aware so a question about *who you are*
/// doesn't have to scan every conversation you ever had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    /// Lived experiences: conversations, events, what happened.
    Episodic,
    /// Consolidated knowledge and facts about the world.
    Semantic,
    /// The deepening model of the user — preferences, traits, projects.
    Identity,
    /// Knowledge *about* skills (their purpose, when to use them).
    Procedural,
}

impl Region {
    pub const ALL: [Region; 4] = [
        Region::Episodic,
        Region::Semantic,
        Region::Identity,
        Region::Procedural,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Region::Episodic => "episodic",
            Region::Semantic => "semantic",
            Region::Identity => "identity",
            Region::Procedural => "procedural",
        }
    }

    /// Which regions are worth consulting for a given task type. This is how the
    /// brain "knows where to look" without searching everything.
    pub fn for_task(task: &str) -> Vec<Region> {
        match task {
            "identity" | "about_user" => vec![Region::Identity, Region::Semantic],
            "qa" | "recall_fact" => vec![Region::Semantic, Region::Identity, Region::Episodic],
            "conversation" | "episodic" => vec![Region::Episodic],
            "skill" | "procedural" => vec![Region::Procedural],
            _ => Region::ALL.to_vec(),
        }
    }
}
