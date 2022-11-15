use std::collections::HashMap;
use std::env;
use std::future::Future;
use std::time::Duration;

use lazy_static::lazy_static;

use serenity::async_trait;
use serenity::model::channel::{Message, Reaction};
use serenity::model::gateway::Ready;
use serenity::model::prelude::{ChannelId, MessageId, UserId};
use serenity::model::user::User;
use serenity::prelude::*;

use tokio::sync::Mutex;

type MoaiMap = HashMap<UserId, usize>;
type UserMap = HashMap<UserId, String>;
type Data = (UserMap, MoaiMap);

const STORAGE_CHANNEL: ChannelId = ChannelId(1029509904203005995);
const STORAGE_MESSAGE: MessageId = MessageId(1041734958122807417);

static mut TASK_QUEUE_COUNT: usize = 0;
static mut DATA_CHANGED: bool = false;

lazy_static! {
    static ref STORAGE: Mutex<DataStorage> = Mutex::new(DataStorage);
    static ref DATA: Mutex<Data> = Mutex::new((UserMap::new(), MoaiMap::new()));
}

struct DataStorage;

impl DataStorage {
    async fn get_data(&self, ctx: &Context) -> Result<Data, Box<dyn std::error::Error>> {
        let data_msg = STORAGE_CHANNEL.message(&ctx.http, STORAGE_MESSAGE).await?;

        Ok(serde_json::from_str(&data_msg.content)?)
    }

    async fn write_data(
        &self,
        ctx: &Context,
        data: Data,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut data_msg = STORAGE_CHANNEL.message(&ctx.http, STORAGE_MESSAGE).await?;

        let data_str = serde_json::to_string(&data)?;

        data_msg.edit(&ctx.http, |m| m.content(data_str)).await?;

        Ok(())
    }
}

async fn task<F, Fut>(f: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(Data) -> Fut,
    Fut: Future<Output = Data>,
{
    unsafe { TASK_QUEUE_COUNT += 1 };
    let mut data_lock = DATA.lock().await;
    let data = f(data_lock.clone()).await;
    *data_lock = data;
    unsafe { TASK_QUEUE_COUNT -= 1 };
    Ok(())
}

async fn leaderboard(ctx: &Context, channel: ChannelId, data: Data) -> Data {
    let (user_map, moai_map) = data;

    let mut leaderboard = user_map
        .iter()
        .map(|(user_id, user_name)| (user_name.clone(), *moai_map.get(user_id).unwrap_or(&0)))
        .collect::<Vec<(String, usize)>>();

    leaderboard.sort_by(|a, b| a.1.cmp(&b.1));

    let everyone = format!(
        "Everyone : {}",
        leaderboard.iter().map(|(_, count)| count).sum::<usize>()
    );

    let individual = leaderboard
        .iter()
        .rev()
        .map(|(name, count)| format!("{} : {}\n", name, count))
        .collect::<String>();

    if let Err(e) = channel
        .say(&ctx.http, format!("{}\n\n{}", everyone, individual))
        .await
    {
        eprintln!("Error sending message: {:?}", e);
    }

    (user_map, moai_map)
}

async fn increment_user(_ctx: &Context, user: User, data: Data) -> Data {
    let (mut user_map, mut moai_map) = data;

    user_map.insert(user.id, user.name);

    let counter = moai_map.get(&user.id).unwrap_or(&0);
    moai_map.insert(user.id, counter + 1);

    unsafe { DATA_CHANGED = true };

    (user_map, moai_map)
}

async fn decrement_user(_ctx: &Context, user: User, data: Data) -> Data {
    let (mut user_map, mut moai_map) = data;

    user_map.insert(user.id, user.name);

    let counter = moai_map.get(&user.id).unwrap_or(&0);
    moai_map.insert(user.id, counter - 1);

    unsafe { DATA_CHANGED = true };

    (user_map, moai_map)
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        match msg.content.as_str() {
            "!leaderboard" => if let Err(e) =
                    task(|data| async move { leaderboard(&ctx, msg.channel_id, data).await }).await
                {
                    eprintln!("Leaderboard task failed {}", e);
                }
            "!debug" => if let Err(e) = msg.channel_id.say(
                        &ctx.http,
                        format!(
                            "{} tasks in queue\nSynced {}\n\nSTORAGE_CHANNEL {}\nSTORAGE_MESSAGE {}\nDATA mutex {}\nSTORAGE mutex {}",
                            unsafe { TASK_QUEUE_COUNT },
                            unsafe { !DATA_CHANGED },
                            STORAGE_CHANNEL,
                            STORAGE_MESSAGE,
                            DATA.try_lock().map_or_else(|_| "locked" , |_| "unlocked"),
                            STORAGE.try_lock().map_or_else(|_| "locked" , |_| "unlocked"),
                        ),
                    )
                    .await
                {
                    eprintln!("Error sending message: {:?}", e);
                }
            "!sync" => {
                println!("Manual sync");
                let message = {
                    let result = STORAGE.lock().await.write_data(&ctx, DATA.lock().await.clone()).await;
                    match result {
                        Ok(_) => { 
                            unsafe { DATA_CHANGED = false };
                            "Successfully synced local and remote".to_string()
                        },
                        Err(e) => format!("Failed to sync data {}", e)
                    }
                };
                if let Err(e) = msg.channel_id
                    .say(&ctx.http, message)
                    .await
                {
                    eprintln!("Error sending message: {:?}", e);
                }
            }
            moyai if moyai.contains(":moyai:") || moyai.contains('🗿') => if let Err(e) =
                    task(|data| async move { increment_user(&ctx, msg.author, data).await }).await
                {
                    eprintln!("Message increment task failed {}", e);
                }
            _ => (),
        };
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        if reaction.emoji.unicode_eq("🗿") {
            let user = match reaction.user(&ctx).await {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Failed to get user for reaction {}", e);
                    return;
                }
            };
            if let Err(e) = task(|data| async move { increment_user(&ctx, user, data).await }).await
            {
                eprintln!("Message increment task failed {}", e);
            }
        }
    }

    async fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        if reaction.emoji.unicode_eq("🗿") {
            let user = match reaction.user(&ctx).await {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Failed to get user for reaction {}", e);
                    return;
                }
            };
            if let Err(e) = task(|data| async move { decrement_user(&ctx, user, data).await }).await
            {
                eprintln!("Message decrement task failed {}", e);
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} ({}) is connected!", ready.user.name, ready.user.id);
        tokio::spawn(async move {
            println!("Background sync task spawned");
            {
                let data = STORAGE
                    .lock()
                    .await
                    .get_data(&ctx)
                    .await
                    .expect("Failed to retrieve data from storage");
                *DATA.lock().await = data;
            }
            println!("Data retrieved from storage");

            loop {
                if unsafe { DATA_CHANGED } {
                    if let Err(e) = STORAGE
                        .lock()
                        .await
                        .write_data(&ctx, DATA.lock().await.clone())
                        .await
                    {
                        eprintln!("Failed to save data {}", e);
                    } else {
                        println!("Data saved");
                    }
                    unsafe { DATA_CHANGED = false };
                }
                tokio::time::sleep(Duration::from_secs(900)).await;
            }
        });
    }
}

#[tokio::main]
async fn main() {
    let token = env::var("DISCORD_TOKEN").expect("Expeced DISCORD_TOKEN in environment");

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    if let Err(e) = client.start().await {
        println!("Client error: {:?}", e);
    }
}
