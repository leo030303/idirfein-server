use std::collections::HashMap;

fn get_client_auth_list() -> HashMap<String, String> {
    // TODO Read this from config
    let mut auth_list = HashMap::new();
    auth_list.insert(String::from("Leo"), String::from("12345"));
    auth_list
}

fn calculate_totp_token(totp_seed: &str) -> String {
    // TODO calculate properly
    totp_seed.to_string()
}

pub fn authorise(client_id: &str, auth_token: &str) -> bool {
    let auth_list = get_client_auth_list();
    if let Some(totp_seed) = auth_list.get(client_id) {
        let server_token = calculate_totp_token(totp_seed);
        auth_token == server_token
    } else {
        false
    }
}
