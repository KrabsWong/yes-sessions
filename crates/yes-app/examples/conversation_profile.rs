use std::time::Instant;
use yes_core::{AppType, ProviderRegistry};
use yes_sessions::conversation::{
    ConversationCache, conversation_turn_count, turn_index_for_message,
};

fn main() -> anyhow::Result<()> {
    let id = std::env::args()
        .nth(1)
        .expect("pass a CodeBuddy session ID");
    let provider = ProviderRegistry::default().get(AppType::CodeBuddy).unwrap();
    let start = Instant::now();
    let detail = provider
        .session_detail_with_usage(&id)?
        .expect("session missing");
    println!(
        "load={:?}, messages={}",
        start.elapsed(),
        detail.messages.len()
    );
    let start = Instant::now();
    let count = conversation_turn_count(&detail.messages, AppType::CodeBuddy);
    println!("group={:?}, turns={count}", start.elapsed());
    let start = Instant::now();
    for (index, _) in detail
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.message_type == yes_core::MessageType::User)
    {
        std::hint::black_box(turn_index_for_message(
            &detail.messages,
            index,
            AppType::CodeBuddy,
        ));
    }
    println!("navigator={:?}", start.elapsed());
    let detail = std::sync::Arc::new(detail);
    let mut cache = ConversationCache::default();
    let start = Instant::now();
    let layout = cache.get(&detail);
    println!("prepare_layout={:?}", start.elapsed());
    let start = Instant::now();
    for (index, _) in detail
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.message_type == yes_core::MessageType::User)
    {
        std::hint::black_box(layout.turn_index_for_message(index));
    }
    println!("indexed_navigator={:?}", start.elapsed());
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(cache.get(&detail));
    }
    println!("cached_layout_average={:?}", start.elapsed() / 1000);
    Ok(())
}
