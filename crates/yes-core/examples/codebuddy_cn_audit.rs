//! Read-only local audit: print counts, never conversation text.
use yes_core::{MessageType, SessionProvider, providers::CodeBuddyCnProvider};
fn main() -> anyhow::Result<()> {
    let provider = CodeBuddyCnProvider::default();
    let started = std::time::Instant::now();
    let sessions = provider.sessions()?;
    let list_time = started.elapsed();
    let mut counts = [0usize; 6];
    let mut user_contexts = 0;
    for session in &sessions {
        let detail = provider
            .session_detail_from_summary(session)?
            .expect("listed session");
        for message in detail.messages {
            if message.message_type == MessageType::User
                && message.metadata.contains_key("user_context")
            {
                user_contexts += 1;
                assert!(message.metadata["original_user_content"].is_string());
            }
            counts[0] += 1;
            counts[1] += usize::from(message.message_type == MessageType::ToolUse);
            counts[2] += usize::from(message.message_type == MessageType::ToolResult);
            counts[3] += message.attachments.len();
            counts[4] += usize::from(message.usage.is_some());
            counts[5] += usize::from(message.reasoning_content.is_some());
        }
    }
    println!(
        "sessions={}, mapped_workspaces={}, source_messages={}, normalized_messages={}, tool_calls={}, tool_results={}, images={}, usage_requests={}, reasoning_messages={}, list={list_time:?}",
        sessions.len(),
        sessions.iter().filter(|s| s.directory.is_some()).count(),
        sessions.iter().map(|s| s.message_count).sum::<usize>(),
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        counts[4],
        counts[5]
    );
    println!("separated_user_contexts={user_contexts}");
    Ok(())
}
