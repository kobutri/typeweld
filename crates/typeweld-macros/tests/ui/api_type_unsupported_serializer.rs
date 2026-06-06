#[derive(typeweld_macros::ApiType)]
struct Profile {
    #[serde(serialize_with = "serialize_display_name")]
    display_name: String,
}

fn serialize_display_name<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(value)
}

fn main() {}
