extern crate serenity;

mod client;
mod db;

use serenity::Client;
use serenity::client::EventHandler;
use serenity::model::id::GuildId;
use serenity::http::GuildPagination;

use std::fs;
use std::io::Write;

struct JWHandler {

}

impl EventHandler for JWHandler {

}

fn main() {
    //client::client_retry().expect("Server Error");
    let conn = db::DBConnection::new().unwrap();
    let db_channels = conn.get_channels().unwrap();
    
    let mut guilds: Vec<u64> = Vec::new();
    let handler = JWHandler {};
    let discord_token = std::env::var("DISCORD_TOKEN").unwrap();
    let client = Client::new(discord_token, handler).unwrap();
    let http = &client.cache_and_http.http;

    let mut channels: Vec<u64> = Vec::new();

    // Fetch all guilds

    // wtf??
    let mut guild_id = GuildId(0);
    loop {
        let mut fetch_guilds: Vec<u64> = match http.get_guilds(&GuildPagination::After(guild_id), 100) {
            Ok(ids) => ids,
            Err(_) => break,
        }.iter().map(|v| v.id.0).collect();
        if fetch_guilds.len() <= 0 { break; }
        guild_id = GuildId(*fetch_guilds.last().unwrap());

        guilds.append(&mut fetch_guilds);
    }
    
    println!("Total Guilds: {}", guilds.len());

    for guild_id in &guilds {
        let mut fetch_channels: Vec<u64> = match http.get_channels(*guild_id) {
            Ok(ids) => ids,
            Err(_) => continue,
        }.iter().map(|v| v.id.0).collect();
        channels.append(&mut fetch_channels);
    }

    let channel_str = channels.iter().fold(String::new(), |acc, x| acc + &x.to_string() + "\n");
    let mut file = fs::File::create("./channels.txt").unwrap();
    file.write_all(channel_str.as_bytes()).unwrap();
}
