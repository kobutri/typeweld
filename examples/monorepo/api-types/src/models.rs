use serde::{Deserialize, Serialize};
use typeweld::Api;

pub type WidgetId = i32;

/// A widget in the catalog.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    pub id: WidgetId,
    pub reference: uuid::Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub part: Part,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub name: String,
    pub tags: Vec<String>,
}
