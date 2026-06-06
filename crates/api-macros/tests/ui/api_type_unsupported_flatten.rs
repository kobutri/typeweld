#[derive(api_macros::ApiType)]
struct Profile {
    #[serde(flatten)]
    details: ProfileDetails,
}

#[derive(api_macros::ApiType)]
struct ProfileDetails {
    display_name: String,
}

fn main() {}
