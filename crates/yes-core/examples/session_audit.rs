use std::time::Instant;

use yes_core::{AppType, ProviderRegistry};

fn main() -> anyhow::Result<()> {
    let registry = ProviderRegistry::default();
    for app_type in AppType::ALL {
        let started = Instant::now();
        let provider = registry
            .get(app_type)
            .ok_or_else(|| anyhow::anyhow!("provider missing: {app_type}"))?;
        let list_started = Instant::now();
        let sessions = provider.sessions()?;
        let list_elapsed = list_started.elapsed();
        let subagents = sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Subagent)
            .count();
        let session_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let orphan_subagents = sessions
            .iter()
            .filter(|session| session.kind == yes_core::model::SessionKind::Subagent)
            .filter(|session| {
                session
                    .parent_session_id
                    .as_deref()
                    .is_none_or(|parent| !session_ids.contains(parent))
            })
            .count();
        let detail_started = Instant::now();
        let detail_messages = sessions
            .first()
            .and_then(|session| provider.session_detail(&session.id).ok().flatten())
            .map(|detail| detail.messages.len())
            .unwrap_or_default();
        println!(
            "{}: available={}, sessions={}, subagents={}, orphan_subagents={}, first_detail_messages={}, list={:.2?}, detail={:.2?}, elapsed={:.2?}",
            app_type,
            provider.is_available(),
            sessions.len(),
            subagents,
            orphan_subagents,
            detail_messages,
            list_elapsed,
            detail_started.elapsed(),
            started.elapsed()
        );
    }
    Ok(())
}
