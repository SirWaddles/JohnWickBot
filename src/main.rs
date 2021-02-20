use serenity::{
    async_trait,
    model::{channel::Message, gateway::Ready},
    prelude::*,
};

use std::env;
use std::sync::Arc;
use chrono::prelude::*;

mod db;
mod broadcast;

struct BotToken {}
impl TypeMapKey for BotToken {
    type Value = String;
}

struct HttpClient {}
impl TypeMapKey for HttpClient {
    type Value = Arc<hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>>;
}

struct DBManager {}
impl TypeMapKey for DBManager {
    type Value = Arc<db::DBManager>;
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: Context, ready: Ready) {
        println!("{} is connected!", ready.user.name);
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!shop" {

            let utc: DateTime<Utc> = Utc::now(); 
            let reply = "https://wickshopbot.com/".to_owned() + &utc.year().to_string() + "_" + &utc.month0().to_string() + "_" + &utc.day().to_string() + ".png";

            if let Err(why) = msg.channel_id.say(&ctx.http, &reply).await {
                println!("Error sending message: {:?}", why);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let token = env::var("DISCORD_TOKEN").expect("token");
    let mut client = Client::builder(&token)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    let https = hyper_tls::HttpsConnector::new();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);
    let db_man = db::DBManager::new().await.unwrap();

    {
        let mut data = client.data.write().await;
        data.insert::<BotToken>(token);
        data.insert::<HttpClient>(Arc::new(http_client));
        data.insert::<DBManager>(Arc::new(db_man));
    }

    println!("Starting Bot");
    if let Err(why) = client.start_shards(8).await {
        println!("An error occurred while running the client: {:?}", why);
    }
}