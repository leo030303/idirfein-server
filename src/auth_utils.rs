use std::{collections::HashMap, fs, path::Path};

use totp_rs::{Algorithm, TOTP};

fn get_client_auth_list() -> HashMap<String, Vec<u8>> {
    let config_file_path = Path::new("/usr/local/share/idirfein_server/users.json");
    let config_json = fs::read_to_string(config_file_path).expect("Couldn't read users file");
    let auth_list: HashMap<String, Vec<u8>> =
        serde_json::from_str(&config_json).expect("Malformed users file");
    auth_list
}

fn calculate_totp_token(totp_seed: &[u8]) -> String {
    let totp =
        TOTP::new(Algorithm::SHA1, 6, 1, 30, totp_seed.to_vec()).expect("Invalid credentials");
    totp.generate_current().expect("Invalid credentials")
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
