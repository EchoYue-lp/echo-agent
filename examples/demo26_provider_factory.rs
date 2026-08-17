//! demo26_provider_factory.rs - explicit provider/model contracts.
//!
//! One application-defined provider connection can host multiple models. Each
//! model selects its own wire protocol and input capabilities.

use echo_agent::prelude::*;

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
        let client = config.build_client()?;
        println!(
            "provider={} model={} protocol={:?} capabilities={:?}",
            config.provider_name.as_deref().unwrap_or("custom"),
            client.model_name(),
            config.api_protocol,
            config.input_modalities,
        );
    }

    Ok(())
}
