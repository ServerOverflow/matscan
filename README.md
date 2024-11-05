# matscan
this is a fork of [matscan](https://github.com/mat-1/matscan) to fit more within ServerOverflow's infrastructure. \
matscan is heavily inspired by [masscan](https://github.com/robertdavidgraham/masscan), and like masscan contains its own tcp stack for maximum speed.

## Changes
- Store exclusions in MongoDB instead of a configuration file and reload them each run
- Small changes to the BSON format data is stored as (so it's less confusing)
- Minor fixes here and there (e.g. only pre-1.13 forge SLP is detected)
- Other miscellaneous changes I can't be bothered to document

## Features
- Adaptive scanning (scans more than just the default port)
- Works well even on relatively low scan rates and with lots of packet drops (running in production at ~70kpps and ~20% loss)
- Can be run in a distributed fashion
- Customizable rescanning (rescan servers with players online more often, etc.)
- Customizable target host, target port, protocol version
- Send to a Discord webhook when a player joins/leaves a server
- Detection of duplicate servers that have the same server on every port
- Protocol implementation fingerprinting (can identify vanilla, paper, fabric, forge, bungeecord, velocity, node-minecraft-protocol)
- Historical player tracking
- Offline-mode detection
- Written in R*st 🚀🚀🚀

## Note
I *highly encourage* you to make your own server scanner instead of relying on someone else's code, I promise you'll have a lot more fun that way. 
Can't really blame you though, as this fork exists only because I didn't want to deep dive into networking... at least for now.
Also, if you do intend on using any of the code here, please read the [license](LICENSE) that the original author wrote.

## Usage
It is assumed that you know the basics of server scanning. Otherwise, I recommend reading the [masscan readme](https://github.com/robertdavidgraham/masscan/blob/master/README.md) and [documentation](https://github.com/robertdavidgraham/masscan/blob/master/doc/masscan.8.markdown).
Also be aware that matscan only supports Linux, but you probably shouldn't be running it at home anyway.

1) Rename `example-config.toml` to `config.toml` and refer to [config.rs](https://github.com/TheAirBlow/matscan/blob/master/src/config.rs) for the format.
2) Create a MongoDB database with all necessary collections and indexes:
```js
use mcscanner
db.createCollection("servers")
db.createCollection("bad_servers")
db.createCollection("exclusions")
db.servers.createIndex({ addr: 1, port: 1 }, { unique: true })
db.servers.createIndex({ timestamp: 1 })
```

3) Populate the exclusions collection from the included `exclude.conf`:
```js
const exclude = fs.readFileSync("exclude.conf", 'utf8');
const lines = exclude.split("\n");
let readComment = false;
let readRanges = false;
let ranges = [];
let comment = [];

for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith("#")) {
        if (readComment && readRanges) {
            db.exclusions.insertOne({
                ranges: ranges,
                comment: comment.join("\n").trim()
            });

            readComment = false;
            readRanges = false;
            ranges = [];
            comment = [];
        }
        
        comment.push(trimmed.slice(1).trim());
        readComment = true;
    } else if (trimmed) {
        ranges.push(trimmed);
        readRanges = true;
    }
}
```

4) Setup iptables, build and start matscan:
```sh
# Firewall port 61000 so your OS doesn't close the connections
# Note: You probably want to use something like iptables-persistent to save this across reboots
iptables -A INPUT -p tcp --dport 61000 -j DROP

# Run in release mode
cargo b -r && sudo ./target/release/matscan
```
