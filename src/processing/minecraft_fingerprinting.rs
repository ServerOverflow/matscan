use std::{
    fmt::Display,
    net::SocketAddrV4,
    sync::{Arc, LazyLock},
    time::SystemTime,
};

use async_trait::async_trait;
use bson::{doc, Bson};
use parking_lot::Mutex;
use regex::Regex;

use crate::{
    config::Config,
    database::{bulk_write::BulkUpdate, Database},
    scanner::protocols,
};

use super::{ProcessableProtocol, SharedData};

static VANILLA_ERROR_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"java\.io\.IOException: Packet (?:\d+|login)\/\d+ \(([^)]+)\)").unwrap()
});

#[derive(Eq, PartialEq)]
enum ServerType {
    Vanilla,
    Fabric,
    Forge,
    Paper,
    NodeMinecraftProtocol,
    Empty,
    Unknown,
}

#[async_trait]
impl ProcessableProtocol for protocols::MinecraftFingerprinting {
    fn process(
        _shared: &Arc<Mutex<SharedData>>,
        _config: &Config,
        target: SocketAddrV4,
        data: &[u8],
        _database: &Database,
    ) -> Option<BulkUpdate> {
        let data_string = String::from_utf8_lossy(data);
        let server_type = if let Some(packet_name) = VANILLA_ERROR_REGEX
            .captures(&data_string)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
        {
            match packet_name {
                "PacketLoginInStart" => ServerType::Paper,
                "ServerboundHelloPacket" => ServerType::Forge,
                _ => {
                    // starts with class_ means fabric
                    if packet_name.starts_with("class_") {
                        ServerType::Fabric
                    }
                    // 2-3 random letters means vanilla
                    else if packet_name.len() >= 2 && packet_name.len() <= 3 {
                        ServerType::Vanilla
                    } else {
                        ServerType::Unknown
                    }
                }
            }
        } else if data_string.contains("Forge") {
            ServerType::Forge
        } else if data.starts_with(&[0x03, 0x03, 0x80, 0x02]) {
            ServerType::NodeMinecraftProtocol
        } else if data.is_empty() {
            ServerType::Empty
        } else {
            ServerType::Unknown
        };

        println!("Fingerprinted {target} as {server_type}: {data_string:?}");

        let mut mongo_update = doc! {
            "fingerprint.active.timestamp": Bson::DateTime(bson::DateTime::from_system_time(SystemTime::now())),
        };
        if server_type != ServerType::Unknown {
            mongo_update.insert(
                "fingerprint.active.software",
                server_type.to_string(),
            );
        }

        Some(BulkUpdate {
            query: doc! {
                "addr": { "$eq": u32::from(*target.ip()) },
                "port": { "$eq": target.port() as u32 }
            },
            update: doc! { "$set": mongo_update },
            options: None,
        })
    }
}

impl Display for ServerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ServerType::Vanilla => "vanilla",
            ServerType::Fabric => "fabric",
            ServerType::Forge => "forge",
            ServerType::Paper => "paper",
            ServerType::NodeMinecraftProtocol => "node_minecraft_protocol",
            ServerType::Empty => "empty",
            ServerType::Unknown => "unknown",
        })
    }
}
