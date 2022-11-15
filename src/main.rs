use std::collections::HashMap;
use std::env;

use lazy_static::lazy_static;

use serenity::async_trait;
use serenity::model::channel::{Message, Reaction};
use serenity::model::gateway::Ready;
use serenity::model::prelude::{ChannelId, MessageId, UserId};
use serenity::model::user::User;
use serenity::prelude::*;

type MoaiMap = HashMap<UserId, usize>;
type UserMap = HashMap<UserId, String>;
type Data = (UserMap, MoaiMap);

const DATA_STORAGE_CHANNEL: ChannelId = ChannelId(1035356938478833734);
const DATA_STORAGE_MSG: MessageId = MessageId(1041738160050278401);

lazy_static! {
    static ref DATA: tokio::sync::Mutex<DataWrapper> = tokio::sync::Mutex::new(DataWrapper);
}

struct DataWrapper;

impl DataWrapper {
    async fn get_data(&self, ctx: &Context) -> Result<Data, Box<dyn std::error::Error>> {
        let data_msg = DATA_STORAGE_CHANNEL
            .message(&ctx.http, DATA_STORAGE_MSG)
            .await?;

        Ok(serde_json::from_str(&data_msg.content)?)
    }

    async fn write_data(
        &self,
        ctx: &Context,
        data: Data,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut data_msg = DATA_STORAGE_CHANNEL
            .message(&ctx.http, DATA_STORAGE_MSG)
            .await?;

        let data_str = serde_json::to_string(&data)?;

        data_msg.edit(&ctx.http, |m| m.content(data_str)).await?;

        Ok(())
    }

    async fn increment_count(&self, ctx: &Context, user: User) {
        let (mut user_map, mut moai_map) = match self.get_data(ctx).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to get data message {}", e);
                return;
            }
        };

        if user_map.get(&user.id).is_none() {
            user_map.insert(user.id, user.name);
        }

        let counter = moai_map.get(&user.id).unwrap_or(&0);
        moai_map.insert(user.id, counter + 1);

        if let Err(e) = self.write_data(ctx, (user_map, moai_map)).await {
            eprintln!("Failed to save data {}", e);
        }
    }
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!leaderboard" {
            let (user_map, moai_map): Data = match DATA.lock().await.get_data(&ctx).await {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Failed to deserialize message {}", e);
                    return;
                }
            };

            let mut leaderboard = user_map
                .iter()
                .map(|(user_id, user_name)| {
                    (user_name.clone(), *moai_map.get(user_id).unwrap_or(&0))
                })
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

            if let Err(e) = msg
                .channel_id
                .say(&ctx.http, format!("{}\n\n{}", everyone, individual))
                .await
            {
                println!("Error sending message: {:?}", e);
            }
        }

        if ctx.cache.current_user_id() != msg.author.id
            && (msg.content.contains(":moyai:") || msg.content.contains('🗿'))
        {
            DATA.lock().await.increment_count(&ctx, msg.author).await;
        }
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
            DATA.lock().await.increment_count(&ctx, user).await;
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("{} ({}) is connected!", ready.user.name, ready.user.id);
        // let data = (UserMap::new(), MoaiMap::new());
        // DATA.lock().await.write_data(&ctx, data);
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
