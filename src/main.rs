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

        if msg.author.id.0 == 229419335930609664 {
            if msg.content.len() >= 10 && &msg.content[..10] == "!broadcast" {
                let data_lock = ctx.data.read().await;
                let (token, http, db) = {
                    let token = data_lock.get::<BotToken>().unwrap().clone();
                    let http = data_lock.get::<HttpClient>().unwrap();
                    let db = data_lock.get::<DBManager>().unwrap();
                    (token, Arc::clone(http), Arc::clone(db))
                };

                tokio::spawn(async move {
                    let channels = match db.get_channels().await {
                        Ok(r) => r,
                        Err(e) => {
                            println!("DB Error: {:#?}", e);
                            return;
                        },
                    };

                    let cast = broadcast::MessageBroadcast::new(db, channels, http, token, &msg.content[11..]);
                    cast.await;
                });
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
    println!("Connecting to Database");
    let db_man = db::DBManager::new().await.unwrap();

    {
        println!("Writing Context");
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