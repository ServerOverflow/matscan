use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    net::SocketAddrV4,
    sync::{Arc, LazyLock},
    time::SystemTime,
};

use anyhow::bail;
use async_trait::async_trait;
use azalea_chat::FormattedText;
use bson::{doc, Bson, Document};
use mongodb::options::UpdateOptions;
use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use tracing::error;

use crate::{
    config::Config,
    database::{self, bulk_write::BulkUpdate, CachedIpHash, Database},
    scanner::protocols,
};

use super::{ProcessableProtocol, SharedData};

const ANONYMOUS_PLAYER_NAME: &str = "Anonymous Player";

#[async_trait]
impl ProcessableProtocol for protocols::Minecraft {
    fn process(
        shared: &Arc<Mutex<SharedData>>,
        config: &Config,
        target: SocketAddrV4,
        data: &[u8],
        database: &Database,
    ) -> Option<BulkUpdate> {
        let data = String::from_utf8_lossy(data);

        let passive_fingerprint = generate_passive_fingerprint(&data).ok();

        let data: serde_json::Value = match serde_json::from_str(&data) {
            Ok(json) => json,
            Err(_) => {
                // not a minecraft server ig
                return None;
            }
        };

        if config.snipe.enabled {
            let mut previous_player_usernames = Vec::new();
            {
                let shared = shared.lock();
                let cached_data = shared.cached_servers.get(&target);
                // Usernames of players that were on the server last time we pinged it
                if let Some(sample) = cached_data
                    .and_then(|s| s.as_object())
                    .and_then(|s| s.get("players"))
                    .and_then(|s| s.as_object())
                    .and_then(|s| s.get("sample"))
                    .and_then(|s| s.as_array())
                {
                    for player in sample {
                        if let Some(player) = player.as_object() {
                            let username = player
                                .get("name")
                                .and_then(|s| s.as_str())
                                .unwrap_or_default();
                            previous_player_usernames.push(username.to_string());
                        }
                    }
                }
            }

            let mut current_player_usernames = Vec::new();

            if let Some(sample) = data
                .as_object()
                .and_then(|s| s.get("players"))
                .and_then(|s| s.as_object())
                .and_then(|s| s.get("sample"))
                .and_then(|s| s.as_array())
            {
                for player in sample {
                    if let Some(player) = player.as_object() {
                        let username = player
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default();

                        current_player_usernames.push(username.to_string());
                    }
                }
            }

            let previous_anon_players_count = previous_player_usernames
                .iter()
                .filter(|&p| p == ANONYMOUS_PLAYER_NAME)
                .count();
            let current_anon_players_count = current_player_usernames
                .iter()
                .filter(|&p| p == ANONYMOUS_PLAYER_NAME)
                .count();

            for current_player in &current_player_usernames {
                if config.snipe.usernames.contains(current_player) {
                    println!("Sniper: {current_player} is in {target}");

                    if !previous_player_usernames.contains(current_player) {
                        tokio::task::spawn(send_to_webhook(
                            config.snipe.webhook_url.clone(),
                            format!("{current_player} joined {target}"),
                        ));
                    }
                }
            }
            for previous_player in &previous_player_usernames {
                if config.snipe.usernames.contains(previous_player)
                    && !current_player_usernames.contains(previous_player)
                {
                    tokio::task::spawn(send_to_webhook(
                        config.snipe.webhook_url.clone(),
                        format!("{previous_player} left {target}"),
                    ));
                }
            }

            if config.snipe.anon_players {
                let version_name = data
                    .as_object()
                    .and_then(|s| s.get("version"))
                    .and_then(|s| s.as_object())
                    .and_then(|s| s.get("name"))
                    .and_then(|s| s.as_str())
                    .unwrap_or_default();
                let online_players = data
                    .as_object()
                    .and_then(|s| s.get("players"))
                    .and_then(|s| s.as_object())
                    .and_then(|s| s.get("online"))
                    .and_then(|s| s.as_i64())
                    .unwrap_or_default();

                let new_anon_players = current_anon_players_count - previous_anon_players_count;

                let meets_new_anon_player_req = !previous_player_usernames.is_empty()
                    && current_anon_players_count > previous_anon_players_count
                    && new_anon_players >= 2;

                let every_online_player_is_anon = current_player_usernames
                    .iter()
                    .all(|p| p == ANONYMOUS_PLAYER_NAME);
                // there's some servers that have a bunch of bots that leave and join, and
                // they're shown as anonymous players in the sample
                let too_many_anon_players =
                    current_anon_players_count >= 8 && every_online_player_is_anon;

                let version_matches = version_name.contains("1.20.4");

                if meets_new_anon_player_req
                    && version_matches
                    && online_players < 25
                    && !too_many_anon_players
                {
                    tokio::task::spawn(send_to_webhook(
                        config.snipe.webhook_url.clone(),
                        format!("{new_anon_players} anonymous players joined **{target}**"),
                    ));
                } else if version_matches
                    && previous_anon_players_count == 0
                    && current_anon_players_count > 0
                    && online_players < 25
                {
                    let webhook_url = config.snipe.webhook_url.clone();
                    let database = database.clone();
                    tokio::task::spawn(async move {
                        // check that there were no anonymous players before
                        let servers_coll = database.servers_coll();
                        let current_data = servers_coll
                            .find_one(doc! {
                                "ip": target.ip().to_string(),
                                "port": target.port() as u32
                            })
                            .await
                            .unwrap_or_default()
                            .unwrap_or_default();

                        let mut historical_player_names = Vec::new();
                        if let Some(sample) =
                            current_data.get("players").and_then(|s| s.as_document())
                        {
                            for (_, player) in sample {
                                if let Some(player) = player.as_document() {
                                    let username = player
                                        .get("name")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or_default();
                                    historical_player_names.push(username.to_string());
                                }
                            }
                        }

                        let has_historical_anon = historical_player_names
                            .iter()
                            .any(|p| p == ANONYMOUS_PLAYER_NAME);

                        if !has_historical_anon {
                            send_to_webhook(
                                webhook_url,
                                format!("anonymous player joined **{target}** for the first time"),
                            )
                            .await;
                        }
                    });
                }
            }

            shared.lock().cached_servers.insert(target, data.clone());
        }

        if let Some(cleaned_data) = clean_response_data(&data, passive_fingerprint) {
            let mongo_update = doc! { "$set": cleaned_data };
            match create_bulk_update(database, &target, mongo_update) {
                Ok(r) => Some(r),
                Err(err) => {
                    error!("Error updating server {target}: {err}");
                    None
                }
            }
        } else {
            None
        }
    }
}

fn decode_optimized(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() < 2 {
        return None;
    }

    let size0 = encoded.chars().nth(0)? as usize;
    let size1 = encoded.chars().nth(1)? as usize;
    let size = size0 | (size1 << 15);

    let mut bytes = Vec::with_capacity(size);
    let mut buffer = 0u32;
    let mut bits_in_buf = 0;
    let chars = encoded.chars().skip(2);

    for c in chars {
        while bits_in_buf >= 8 {
            bytes.push((buffer & 0xFF) as u8);
            buffer >>= 8;
            bits_in_buf -= 8;
        }

        buffer |= ((c as u32) & 0x7FFF) << bits_in_buf;
        bits_in_buf += 15;
    }

    while bytes.len() < size && bits_in_buf >= 8 {
        bytes.push((buffer & 0xFF) as u8);
        buffer >>= 8;
        bits_in_buf -= 8;
    }

    Some(bytes)
}

fn read_varint(bytes: &mut &[u8]) -> Option<u32> {
    let mut num = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        let byte = *bytes.get(0)?;
        *bytes = &bytes[1..];
        num |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Some(num);
        }
        shift += 7;
    }
    None
}

fn read_utf(bytes: &mut &[u8]) -> Option<String> {
    let len = read_varint(bytes)? as usize;
    if bytes.len() < len {
        return None;
    }
    let string = std::str::from_utf8(&bytes[..len]).ok()?.to_string();
    *bytes = &bytes[len..];
    Some(string)
}

fn read_bool(bytes: &mut &[u8]) -> Option<bool> {
    let byte = *bytes.get(0)?;
    *bytes = &bytes[1..];
    Some(byte != 0)
}

fn read_u16(bytes: &mut &[u8]) -> Option<u16> {
    if bytes.len() < 2 {
        return None;
    }
    let val = u16::from_be_bytes([bytes[0], bytes[1]]);
    *bytes = &bytes[2..];
    Some(val)
}

fn extract_forge_mods(encoded: &str) -> Option<Vec<Bson>> {
    let decoded_vec = decode_optimized(encoded)?;
    let mut bytes = decoded_vec.as_slice();

    let _truncated = read_bool(&mut bytes)?;
    let mods_size = read_u16(&mut bytes)? as usize;

    let mut mods = Vec::new();

    for _ in 0..mods_size {
        let channel_size_and_flag = read_varint(&mut bytes)?;
        let channel_size = channel_size_and_flag >> 1;
        let is_ignore_server_only = (channel_size_and_flag & 1) != 0;

        let mod_id = read_utf(&mut bytes)?;
        let mod_version = if is_ignore_server_only {
            "IGNORESERVERONLY".to_string()
        } else {
            read_utf(&mut bytes)?
        };

        for _ in 0..channel_size {
            let _channel_name = read_utf(&mut bytes)?;
            let _channel_version = read_utf(&mut bytes)?;
            let _required = read_bool(&mut bytes)?;
            // we ignore channels for now
        }

        let mut mod_doc = Document::new();
        mod_doc.insert("modId", Bson::String(mod_id));
        mod_doc.insert("modmarker", Bson::String(mod_version));
        mods.push(Bson::Document(mod_doc));
    }

    Some(mods)
}

/// Clean up the response data from the server into something we can insert into
/// our database.
fn clean_response_data(
    data: &serde_json::Value,
    passive_minecraft_fingerprint: Option<PassiveMinecraftFingerprint>,
) -> Option<Document> {
    let json = data.as_object()?.to_owned();
    let mut data = Bson::deserialize(data).ok()?;
    let mut data = data.as_document_mut()?.to_owned();
    let Some(description) = json.get("description")
        //.map(|d| FormattedText::deserialize(d).unwrap_or_default())
    else {
        // no description, so probably not even a minecraft server
        return None;
    };

    data.insert("description", Bson::String(description.to_string()));

    if let Some(clean_description) = json.get("description")
        .map(|d| FormattedText::deserialize(d).unwrap_or_default()) {
        data.insert("cleanDescription", Bson::String(clean_description.to_string()));
    }

    // forge stuff
    let legacy_forge = data.contains_key("modinfo");
    let new_forge = data.contains_key("forgeData");
    data.insert("isForge", legacy_forge || new_forge);

    if let Some(forge_data) = data.get_mut("forgeData").and_then(Bson::as_document_mut) {
        if let Some(d_val) = forge_data.get("d").and_then(Bson::as_str) {
            if let Some(mods) = extract_forge_mods(d_val) {
                forge_data.insert("mods", Bson::Array(mods));
            }
        }
    }

    // This is going to be handled on the frontend for me to be able to update the list at any time
    /*let version_name = data
        .get("version")
        .and_then(|v| v.as_document())
        .and_then(|v| v.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or_default();

    if description.contains("Craftserve.pl - wydajny hosting Minecraft!")
        || description.contains("Ochrona DDoS: Przekroczono limit polaczen.")
        || description.contains("¨ |  ")
        || description.contains("Start the server at FalixNodes.net/start")
        || description.contains("This server is offline Powered by FalixNodes.net")
        || description.contains("Serwer jest aktualnie wy")
        || description.contains("Blad pobierania statusu. Polacz sie bezposrednio!")
        || matches!(
            version_name,
            "COSMIC GUARD" | "TCPShield.com" | "â  Error" | "⚠ Error"
        )
    {
        return None
    }*/

    let mut is_online_mode: Option<bool> = None;
    let mut mixed_online_mode = false;
    let mut fake_sample = false;
    let mut has_players = false;

    let mut players_data = Document::default();

    // servers with this MOTD randomize the online players
    let player_list_hidden = description == "To protect the privacy of this server and its\nusers, you must log in once to see ping data.";

    if !player_list_hidden {
        for player in data
            .get("players")
            .and_then(|p| p.as_document())
            .and_then(|p| p.get("sample"))
            .and_then(|p| p.as_array())
            .map(|s| s.iter().take(100).collect::<Vec<_>>())
            .unwrap_or_default()
        {
            let player = player.as_document()?;

            let uuid = player
                .get("id")
                .and_then(|id| id.as_str())
                .unwrap_or_default();
            let name = player
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();

            let uuid = uuid.replace('-', "");

            static UUID_REGEX: LazyLock<Regex> =
                LazyLock::new(|| Regex::new("[0-9a-f]{12}[34][0-9a-f]{19}").unwrap());

            // anonymous player is a nil uuid so it wouldn't match the regex
            if !UUID_REGEX.is_match(&uuid) && name != ANONYMOUS_PLAYER_NAME {
                fake_sample = true;
            }

            if !mixed_online_mode {
                // ignore nil uuids (anonymous players)
                let is_nil = uuid.chars().all(|c| c == '0');
                if !is_nil {
                    // uuidv4 means online mode
                    // uuidv3 means offline mode
                    let is_uuidv4 = uuid.len() >= 12 && uuid[12..].starts_with('4');
                    if (is_uuidv4 && is_online_mode == Some(false))
                        || (!is_uuidv4 && is_online_mode == Some(true))
                    {
                        mixed_online_mode = true;
                    } else if is_online_mode.is_none() {
                        is_online_mode = Some(is_uuidv4);
                    }
                }
            }

            let mut player_doc = Document::new();
            player_doc.insert(
                "lastSeen",
                Bson::DateTime(bson::DateTime::from_system_time(SystemTime::now())),
            );
            player_doc.insert("name", Bson::String(name.to_string()));

            players_data.insert(format!("players.{}", uuid), player_doc);

            has_players = true;
        }
    }

    let mut final_cleaned = doc! {
        "timestamp": bson::DateTime::from_system_time(SystemTime::now()),
        "minecraft": data,
    };

    // C# enums are serialized as an int32 inside the BSON document
    if mixed_online_mode {
        final_cleaned.insert("onlineModeGuess", Bson::Int32(2)); // mixed
    } else if let Some(_) = is_online_mode {
        final_cleaned.insert("onlineModeGuess", Bson::Int32(1)); // online
    } else {
        final_cleaned.insert("onlineModeGuess", Bson::Int32(0)); // offline
    }

    if !fake_sample {
        final_cleaned.extend(players_data);
        if has_players {
            final_cleaned.insert(
                "lastActive",
                Bson::DateTime(bson::DateTime::from_system_time(SystemTime::now())),
            );
        } else {
            final_cleaned.insert(
                "lastEmpty",
                Bson::DateTime(bson::DateTime::from_system_time(SystemTime::now())),
            );
        }
    }

    if let Some(passive_minecraft_fingerprint) = passive_minecraft_fingerprint {
        final_cleaned.insert(
            "fingerprint.passive.incorrectOrder",
            Bson::Boolean(passive_minecraft_fingerprint.incorrect_order),
        );
        if let Some(field_order) = passive_minecraft_fingerprint.field_order {
            final_cleaned.insert(
                "fingerprint.passive.fieldOrder",
                Bson::String(field_order),
            );
        }
        final_cleaned.insert(
            "fingerprint.passive.emptySample",
            Bson::Boolean(passive_minecraft_fingerprint.empty_sample),
        );
        final_cleaned.insert(
            "fingerprint.passive.emptyFavicon",
            Bson::Boolean(passive_minecraft_fingerprint.empty_favicon),
        );
    }

    Some(final_cleaned)
}

pub fn create_bulk_update(
    database: &Database,
    target: &SocketAddrV4,
    mongo_update: Document,
) -> anyhow::Result<BulkUpdate> {
    if database.shared.lock().bad_ips.contains(target.ip()) && target.port() != 25565 {
        // no
        bail!("bad ip");
    }

    fn determine_hash(mongo_update: &Document) -> anyhow::Result<u64> {
        let set_data = mongo_update.get_document("$set")?;
        let minecraft = set_data.get_document("minecraft")?;

        let version = minecraft.get_document("version")?;

        let description = minecraft.get_str("description").unwrap_or_default();
        let version_name = version.get_str("name").unwrap_or_default();
        let version_protocol = database::get_i32(version, "protocol").unwrap_or_default();
        let max_players = minecraft
            .get_document("players")
            .ok()
            .and_then(|p| database::get_i32(p, "max"))
            .unwrap_or_default();

        let mut hasher = DefaultHasher::new();
        (description, version_name, version_protocol, max_players).hash(&mut hasher);
        Ok(hasher.finish())
    }

    let mut is_bad_ip = false;
    let mut shared = database.shared.lock();
    let ips_with_same_hash = shared.ips_with_same_hash.get_mut(target.ip());
    if let Some((data, previously_checked_ports)) = ips_with_same_hash {
        if !previously_checked_ports.contains(&target.port()) {
            if let Some(count) = &mut data.count {
                let this_server_hash = determine_hash(&mongo_update)?;

                if this_server_hash == data.hash {
                    *count += 1;
                    previously_checked_ports.insert(target.port());

                    if *count >= 100 {
                        // too many servers with the same hash... add to bad ips!
                        println!("Found a new bad IP: {}", target.ip());
                        // calls add_to_bad_ips slightly lower down
                        // we have to do it like that to avoid keeping the lock during await
                        is_bad_ip = true;
                    }
                } else {
                    // this server has a different hash than the other servers with the same IP
                    data.count = None;
                }
            }
        }
    } else {
        let this_server_hash = determine_hash(&mongo_update)?;
        shared.ips_with_same_hash.insert(
            *target.ip(),
            (
                CachedIpHash {
                    count: Some(1),
                    hash: this_server_hash,
                },
                HashSet::from_iter(vec![target.port()]),
            ),
        );
    }

    if is_bad_ip {
        tokio::spawn(database.to_owned().add_to_bad_ips(*target.ip()));
        bail!("bad ip {target:?}");
    }

    Ok(BulkUpdate {
        query: doc! {
            "ip": { "$eq": target.ip().to_string() },
            "port": { "$eq": target.port() as u32 }
        },
        update: mongo_update,
        options: Some(UpdateOptions::builder().upsert(true).build()),
    })
}

async fn send_to_webhook(webhook_url: String, message: String) {
    let client = reqwest::Client::new();
    if let Err(e) = client
        .post(webhook_url)
        .json(
            &vec![("content".to_string(), message.to_string())]
                .into_iter()
                .collect::<HashMap<String, String>>(),
        )
        .send()
        .await
    {
        println!("Failed to send a webhook message: {}", e);
    }
}

pub struct PassiveMinecraftFingerprint {
    pub incorrect_order: bool,
    pub field_order: Option<String>,
    /// Servers shouldn't have the sample field if there are no players online.
    pub empty_sample: bool,
    /// A favicon that has the string ""
    pub empty_favicon: bool,
}
pub fn generate_passive_fingerprint(data: &str) -> anyhow::Result<PassiveMinecraftFingerprint> {
    let data: serde_json::Value = serde_json::from_str(data)?;

    let protocol_version = data
        .get("version")
        .and_then(|s| s.as_object())
        .and_then(|s| s.get("protocol"))
        .and_then(|s| s.as_u64())
        .unwrap_or_default();

    let empty_favicon = data.get("favicon").map(|s| s.as_str()) == Some(Some(""));

    let mut incorrect_order = false;
    let mut field_order = None;
    let mut empty_sample = false;

    // the correct field order is description, players, version (ignore everything
    // else)

    if let Some(data) = data.as_object() {
        // mojang changed the order in 23w07a/1.19.4
        let correct_order = if matches!(protocol_version, 1073741943.. | 762..=0x40000000 ) {
            ["version", "description", "players"]
        } else {
            ["description", "players", "version"]
        };

        let keys = data
            .keys()
            .filter(|&k| correct_order.contains(&k.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        let players = data.get("players").and_then(|s| s.as_object());
        let version = data.get("version").and_then(|s| s.as_object());

        let correct_players_order = ["max", "online"];
        let correct_version_order = ["name", "protocol"];

        let players_keys = players
            .map(|s| {
                s.keys()
                    .filter(|&k| correct_players_order.contains(&k.as_str()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let version_keys = version
            .map(|s| {
                s.keys()
                    .filter(|&k| correct_version_order.contains(&k.as_str()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if keys != correct_order
            || players_keys != correct_players_order
            || version_keys != correct_version_order
        {
            incorrect_order = true;
        }

        if incorrect_order {
            let mut field_order_string = String::new();
            for (i, key) in keys.iter().enumerate() {
                field_order_string.push_str(key);
                if key == "players" && players_keys != correct_players_order {
                    field_order_string.push_str(format!("({})", players_keys.join(",")).as_str());
                } else if key == "version" && version_keys != correct_version_order {
                    field_order_string.push_str(format!("({})", version_keys.join(",")).as_str());
                }
                if i != keys.len() - 1 {
                    field_order_string.push(',');
                }
            }
            field_order = Some(field_order_string);
        }

        if let Some(players) = data.get("players").and_then(|s| s.as_object()) {
            if let Some(sample) = players.get("sample").and_then(|s| s.as_array()) {
                if sample.is_empty() {
                    empty_sample = true;
                }
            }
        }
    }

    Ok(PassiveMinecraftFingerprint {
        incorrect_order,
        field_order,
        empty_sample,
        empty_favicon,
    })
}
