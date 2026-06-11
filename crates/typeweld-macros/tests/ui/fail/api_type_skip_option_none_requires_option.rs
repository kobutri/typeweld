use typeweld::Api;

#[derive(Clone, Debug, Api)]
pub struct Bad {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: String,
}

fn main() {}
