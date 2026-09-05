use crate::error::{ReactError, Result};
use crate::llm::types::{ContentPart, LinkedResource, Message};
use agent_client_protocol::schema::v1::ContentBlock;

pub(crate) enum MappedPrompt {
    Text(String),
    Structured(Message),
}

pub(crate) fn map_prompt(blocks: Vec<ContentBlock>, max_chars: usize) -> Result<MappedPrompt> {
    if blocks.is_empty() {
        return Err(ReactError::Other(
            "ACP Prompt must contain at least one content block".to_string(),
        ));
    }
    let mut parts = Vec::with_capacity(blocks.len());
    let mut has_resource_link = false;
    let mut total_chars = 0usize;
    for block in blocks {
        let (part, measured) = match block {
            ContentBlock::Text(text) => {
                let measured = text.text.chars().count();
                (ContentPart::Text { text: text.text }, measured)
            }
            ContentBlock::ResourceLink(link) => {
                let encoded = serde_json::to_string(&link).map_err(|error| {
                    ReactError::Other(format!("failed to encode ACP ResourceLink: {error}"))
                })?;
                has_resource_link = true;
                let annotations = link
                    .annotations
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| {
                        ReactError::Other(format!(
                            "failed to preserve ACP ResourceLink annotations: {error}"
                        ))
                    })?;
                let meta = link
                    .meta
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| {
                        ReactError::Other(format!(
                            "failed to preserve ACP ResourceLink metadata: {error}"
                        ))
                    })?;
                (
                    ContentPart::ResourceLink {
                        resource: Box::new(LinkedResource {
                            annotations,
                            description: link.description,
                            mime_type: link.mime_type,
                            name: link.name,
                            size: link.size,
                            title: link.title,
                            uri: link.uri,
                            meta,
                        }),
                    },
                    encoded.chars().count(),
                )
            }
            _ => {
                return Err(ReactError::Other(
                    "ACP Prompt content type was not negotiated".to_string(),
                ));
            }
        };
        total_chars = total_chars
            .checked_add(measured)
            .and_then(|count| count.checked_add(2))
            .ok_or_else(|| ReactError::Other("ACP Prompt size overflow".to_string()))?;
        if total_chars > max_chars {
            return Err(ReactError::Other(format!(
                "ACP Prompt exceeds the configured {max_chars} character limit"
            )));
        }
        parts.push(part);
    }
    if has_resource_link {
        Ok(MappedPrompt::Structured(Message::user_multimodal(parts)))
    } else {
        let text = parts
            .into_iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(MappedPrompt::Text(text))
    }
}
