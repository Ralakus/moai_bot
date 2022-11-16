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

// Type aliases instead of structs to keep things simple
type MoaiMap = HashMap<UserId, usize>;
type UserMap = HashMap<UserId, String>;
type Data = (UserMap, MoaiMap);

// The persistent data is stored in a Discord message that is edited and updated by the bot
const STORAGE_CHANNEL: ChannelId = ChannelId(1029509904203005995);
const STORAGE_MESSAGE: MessageId = MessageId(1041734958122807417);

// Globals for tracking current state of the program
static mut TASK_QUEUE_COUNT: usize = 0;
static mut DATA_CHANGED: bool = false;

// Global mutex locked Data. Mutexes are to prevent data races.
// In `lazy_static` due to tokio's mutex not having a const constructor
lazy_static! {
    /// The accessor for interacting with the data on Discord
    static ref STORAGE: Mutex<DataStorage> = Mutex::new(DataStorage);

    /// The local data cache
    static ref DATA: Mutex<Data> = Mutex::new((UserMap::new(), MoaiMap::new()));
}

/// Wrapper around the Discord message get and edit to avoid data race conditions
struct DataStorage;

impl DataStorage {
    /// Gets data from Discord message and parses the json into the global Data type
    async fn get_data(&self, ctx: &Context) -> Result<Data, Box<dyn std::error::Error>> {
        // Get message from Discord
        let data_msg = STORAGE_CHANNEL.message(&ctx.http, STORAGE_MESSAGE).await?;

        // Parse json contents
        Ok(serde_json::from_str(&data_msg.content)?)
    }

    /// Saves the inputted data into the Discord message as json
    async fn write_data(
        &self,
        ctx: &Context,
        data: Data,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Get message from Discord
        let mut data_msg = STORAGE_CHANNEL.message(&ctx.http, STORAGE_MESSAGE).await?;

        // Convert local data cache to json
        let data_str = serde_json::to_string(&data)?;

        // Edit the message in Discord setting the contents to the new json string
        data_msg.edit(&ctx.http, |m| m.content(data_str)).await?;

        // Ok 👍
        Ok(())
    }
}

/// Wrapper that makes it easy to access data by passing in a closure that takes the data
/// and automatically locking the data mutex
async fn task<F, Fut>(f: F)
where
    F: FnOnce(Data) -> Fut,
    Fut: Future<Output = Data>,
{
    // Increment task count
    unsafe { TASK_QUEUE_COUNT += 1 };

    // Wait until data mutex is available
    let mut data_lock = DATA.lock().await;

    // Run passed in function with data
    let data = f(data_lock.clone()).await;

    // Update the local data with result of the function
    *data_lock = data;

    // Decrement task count
    unsafe { TASK_QUEUE_COUNT -= 1 };
}

/// `!leaderboard` command output
async fn leaderboard(ctx: &Context, channel: ChannelId, data: Data) -> Data {
    let (user_map, moai_map) = data;

    // Get a vector of username and moai count tuples
    let mut leaderboard = user_map
        .iter()
        .map(|(user_id, user_name)| (user_name.clone(), *moai_map.get(user_id).unwrap_or(&0)))
        .collect::<Vec<(String, usize)>>();

    // Sort leaderboard least to greatest
    leaderboard.sort_by(|a, b| a.1.cmp(&b.1));

    // Total everyone's counts
    let everyone = format!(
        "Everyone : {}",
        leaderboard.iter().map(|(_, count)| count).sum::<usize>()
    );

    // Generate each person's count. Iterator is reversed so leaderboard is greatest to least
    let individual = leaderboard
        .iter()
        .rev()
        .map(|(name, count)| format!("{} : {}\n", name, count))
        .collect::<String>();

    // Send leaderboard message
    if let Err(e) = channel
        .say(&ctx.http, format!("{}\n\n{}", everyone, individual))
        .await
    {
        eprintln!("Error sending message: {:?}", e);
    }

    (user_map, moai_map)
}

/// Increments a user's moai count
async fn increment_user(_ctx: &Context, user: User, data: Data) -> Data {
    // Destructure data into the two halves
    let (mut user_map, mut moai_map) = data;

    // Pairs the user id with the username if not already added
    user_map.insert(user.id, user.name);

    // Gets the user's count and increments it
    let counter = moai_map.get(&user.id).unwrap_or(&0);
    moai_map.insert(user.id, counter + 1);

    // Set data change flag
    unsafe { DATA_CHANGED = true };

    // Return updated data
    (user_map, moai_map)
}

/// Decrements a user's moai count
async fn decrement_user(_ctx: &Context, user: User, data: Data) -> Data {
    // Destructure data into the two halves
    let (mut user_map, mut moai_map) = data;

    // Pairs the user id with the username if not already added
    user_map.insert(user.id, user.name);

    // Gets the user's count and increments it
    let counter = moai_map.get(&user.id).unwrap_or(&0);
    moai_map.insert(user.id, counter - 1);

    // Set data change flag
    unsafe { DATA_CHANGED = true };

    // Return updated data
    (user_map, moai_map)
}

/// Serenity Discord api handler
struct Handler;

#[async_trait]
impl EventHandler for Handler {
    /// Function is called whenever a message is sent that is visible to the bot
    async fn message(&self, ctx: Context, msg: Message) {
        match msg.content.as_str() {

            // Outputs the leaderboard of everyone's count
            "!leaderboard" => task(|data| async move { leaderboard(&ctx, msg.channel_id, data).await }).await,

            // Prints out debug information that includes the current state of the bot
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

            // Writes local data cache to Discord
            "!sync" => {
                println!("Manual sync");

                // Message to be sent in Discord
                let message = {
                    // Result is in subscope to avoid holding the mutex lock across an await point
                    let result = STORAGE.lock().await.write_data(&ctx, DATA.lock().await.clone()).await;

                    // Return message
                    match result {
                        Ok(_) => {
                            // Resets data change flag
                            unsafe { DATA_CHANGED = false };
                            "Successfully synced local and remote".to_string()
                        },
                        Err(e) => format!("Failed to sync data {}", e)
                    }
                };

                // Send message
                if let Err(e) = msg.channel_id
                    .say(&ctx.http, message)
                    .await
                {
                    eprintln!("Error sending message: {:?}", e);
                }
            }

            // Increment user's count if the a moyai is found
            moyai if moyai.contains(":moyai:") || moyai.contains('🗿') => {
                task(|data| async move { increment_user(&ctx, msg.author, data).await }).await;
            }
            _ => (),
        };
    }

    /// Function is called whenever a reaction is added that is visible to the bot
    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        if reaction.emoji.unicode_eq("🗿") {
            // Call Discord's api with reaction's user id since the
            // reaction doesn't actually include user information
            let user = match reaction.user(&ctx).await {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Failed to get user for reaction {}", e);
                    return;
                }
            };

            // Run increment task
            task(|data| async move { increment_user(&ctx, user, data).await }).await;
        }
    }

    /// Function is called whenever a reaction is removed that is visible to the bot
    async fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        if reaction.emoji.unicode_eq("🗿") {
            // Call Discord's api with reaction's user id since the
            // reaction doesn't actually include user information
            let user = match reaction.user(&ctx).await {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Failed to get user for reaction {}", e);
                    return;
                }
            };

            // Run decrement task
            task(|data| async move { decrement_user(&ctx, user, data).await }).await;
        }
    }

    /// Function is called once upon startup after the bot is connected
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("{} ({}) is connected!", ready.user.name, ready.user.id);

        // Background task for syncing the local and Discord data
        tokio::spawn(async move {
            println!("Background sync task spawned");

            // When task firsts starts, pull data from Discord and copy it to local cache
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

            // Every 900 seconds, check if the local data cache was changed if it was, update
            // the data on Discord
            loop {
                // Only update the message if it has changed since the last sync
                if unsafe { DATA_CHANGED } {
                    // Write data to Discord
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

                    // Reset data change flag
                    unsafe { DATA_CHANGED = false };
                }

                // Wait
                tokio::time::sleep(Duration::from_secs(900)).await;
            }
        });
    }
}

// Entry point of server
#[tokio::main]
async fn main() {
    // Discord API token
    let token = env::var("DISCORD_TOKEN").expect("Expeced DISCORD_TOKEN in environment");

    // Discord bot intents to recieve events for
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Bot client
    let mut client = Client::builder(token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    // Run bot
    if let Err(e) = client.start().await {
        println!("Client error: {:?}", e);
    }
}
