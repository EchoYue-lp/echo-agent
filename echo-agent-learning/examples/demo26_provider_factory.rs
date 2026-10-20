//! demo26_provider_factory.rs - explicit provider/model contracts.
//!
//! One application-defined provider connection can host multiple models. Each
//! model selects its own wire protocol and input capabilities.

use echo_agent::prelude::*;
use std::sync::Arc;

fn main() -> echo_agent::error::Result<()> {
    let provider_id = "company-gateway";
    let api_root = "https://gateway.example/v1";
    let api_key = "example-key";

    let text_model = LlmConfig::for_provider(
        provider_id,
        api_root,
        api_key,
        "reasoning-model",
        LlmApiProtocol::Responses,
    )?;
    let vision_model = LlmConfig::for_provider(
        provider_id,
        api_root,
        api_key,
        "vision-model",
        LlmApiProtocol::ChatCompletions,
    )?
    .with_input_modalities(vec![ModelInputModality::Text, ModelInputModality::Image]);
    let audio_video_model = LlmConfig::for_provider(
        provider_id,
        api_root,
        api_key,
        "media-model",
        LlmApiProtocol::Anthropic,
    )?
    .with_input_modalities(vec![
        ModelInputModality::Text,
        ModelInputModality::Audio,
        ModelInputModality::Video,
    ]);

    for config in [text_model, vision_model, audio_video_model] {
        let client: Arc<dyn LlmClient> = Arc::from(config.build_client()?);
        println!(
            "provider={} model={} protocol={:?} capabilities={:?}",
            config.provider_name.as_deref().unwrap_or("custom"),
            client.model_name(),
            config.api_protocol,
            config.input_modalities,
        );
        // Retain the prepared client/config through the full Builder path.
        // Construction performs no model request.
        let agent = ReactAgentBuilder::new()
            .configured_llm_client(Arc::clone(&client), config)
            .build_strict()?;
        assert!(
            agent
                .llm_client()
                .is_some_and(|value| Arc::ptr_eq(value, &client))
        );
        assert!(agent.llm_config().is_some());
    }

    Ok(())
}
