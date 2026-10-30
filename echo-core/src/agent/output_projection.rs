//! 当前原生 Agent 流的纯输出归并，不拥有执行、取消、持久化或物理结算。
//!
//! 每个实例只观察一条原 producer 流。调用方必须继续读取到真实 EOF，之后才调用
//! `finish`；块 complete、FinalAnswer 确认和 foreign terminal 都不能替代该读取。

use super::{AssistantMessageBlock, AssistantPhase, ExecutionStatus, ExecutionStatusKind};
use crate::error::{ReactError, Result};
use crate::llm::types::{ProviderOutputSource, ReasoningBlock};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct AssistantTextDelta {
    pub block: AssistantMessageBlock,
    pub text: String,
    pub reasoning_started: bool,
    pub reasoning_finished: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ReasoningOrigin {
    CanonicalBlock(String),
    NativeItem(String),
}

#[derive(Default)]
struct BlockProjection {
    reasoning_seen: bool,
    complete: bool,
    phase: Option<AssistantPhase>,
    text: String,
    source: Option<ProviderOutputSource>,
    reasoning_coordinate: Option<(ReasoningOrigin, String, usize)>,
}

/// 只保存本流已观察事实，不从旧事件或缺字段资料补造 owner/phase/成功。
#[derive(Default)]
pub struct AgentOutputProjection {
    execution_id: Option<String>,
    statuses: BTreeMap<u64, ExecutionStatus>,
    observations: BTreeMap<(String, String, u64), AssistantMessageBlock>,
    blocks: BTreeMap<(String, String), BlockProjection>,
    confirmed_answer: Option<String>,
    final_presentation_taken: bool,
    reasoning_parts: BTreeMap<(String, ReasoningOrigin, String, usize), String>,
    terminal: Option<ExecutionStatus>,
}

fn invalid(message: &str) -> ReactError {
    ReactError::Other(format!("invalid canonical Agent output: {message}"))
}

impl AgentOutputProjection {
    /// Original source identity, exposed only after its Started was validated.
    pub fn execution_id(&self) -> Option<&str> {
        self.execution_id.as_deref()
    }

    pub fn observe_status(&mut self, status: &ExecutionStatus) -> Result<bool> {
        if status.adapter_generated {
            return Err(invalid("retired wrapper-generated execution status"));
        }
        if status.execution_id.trim().is_empty() {
            return Err(invalid("empty execution identity"));
        }
        match self.execution_id.as_deref() {
            Some(execution) if execution != status.execution_id => {
                return Err(invalid("foreign execution status on one producer stream"));
            }
            None if status.status != ExecutionStatusKind::Started => {
                return Err(invalid("native source must start with its own Started"));
            }
            None => self.execution_id = Some(status.execution_id.clone()),
            _ => {}
        }
        if let Some(previous) = self.statuses.get(&status.sequence) {
            return if previous == status {
                Ok(false)
            } else {
                Err(invalid("status identity reused with different payload"))
            };
        }
        if self.terminal.is_some() {
            return Err(invalid("new status after runtime terminal"));
        }
        if self
            .statuses
            .last_key_value()
            .is_some_and(|(sequence, _)| status.sequence <= *sequence)
        {
            return Err(invalid("runtime status sequence regressed"));
        }
        if !self.statuses.is_empty() && status.status == ExecutionStatusKind::Started {
            return Err(invalid("runtime execution started twice"));
        }
        if matches!(
            status.status,
            ExecutionStatusKind::Settled
                | ExecutionStatusKind::Failed
                | ExecutionStatusKind::Cancelled
        ) {
            self.terminal = Some(status.clone());
        }
        self.statuses.insert(status.sequence, status.clone());
        Ok(true)
    }

    pub fn observe_block(
        &mut self,
        block: &AssistantMessageBlock,
    ) -> Result<Option<AssistantTextDelta>> {
        if self.execution_id.is_none() || self.terminal.is_some() {
            return Err(invalid("assistant block outside active native execution"));
        }
        if block.request_id.trim().is_empty() || block.block_id.trim().is_empty() {
            return Err(invalid("assistant block lacks request/block identity"));
        }
        let observation = (
            block.request_id.clone(),
            block.block_id.clone(),
            block.sequence,
        );
        if let Some(previous) = self.observations.get(&observation) {
            return if previous == block {
                Ok(None)
            } else {
                Err(invalid("block observation reused with different payload"))
            };
        }
        let reasoning = block.phase == AssistantPhase::Reasoning;
        let mut text = block.delta.clone();
        let canonical_key = (block.request_id.clone(), block.block_id.clone());
        let previous_state = self.blocks.get(&canonical_key);
        let previous_source = previous_state.and_then(|state| state.source.as_ref());
        if let (Some(previous), Some(current)) = (previous_source, block.provider_source.as_ref())
            && (previous.protocol != current.protocol
                || previous.request_id != current.request_id
                || previous.item_id != current.item_id
                || previous.output_index != current.output_index
                || previous.content_index != current.content_index
                || previous.content_kind != current.content_kind)
        {
            return Err(invalid(
                "canonical block changed its known native coordinates",
            ));
        }
        let effective_source = block.provider_source.as_ref().or(previous_source);
        let previous_coordinate =
            previous_state.and_then(|state| state.reasoning_coordinate.clone());
        let mut reasoning_coordinate = None;
        if reasoning {
            let item = effective_source
                .map(|source| ReasoningOrigin::NativeItem(source.item_id.clone()))
                // 未知native来源的前缀仅属于此canonical block，不能与同名native item碰撞。
                .unwrap_or_else(|| ReasoningOrigin::CanonicalBlock(block.block_id.clone()));
            let kind = if effective_source.and_then(|source| source.content_kind.as_deref())
                == Some("summary_text")
            {
                "summary"
            } else {
                "thinking"
            };
            let part = effective_source
                .and_then(|source| source.content_index)
                .map(|index| index as usize)
                .unwrap_or(0);
            let coordinate = (item.clone(), kind.to_string(), part);
            // 原canonical block先有正文、后有native来源时，只搬移已呈现前缀。
            // 补全依据同request/block及真实坐标，不按文字相似度跨请求去重。
            if let Some(previous) = previous_coordinate
                && previous != coordinate
                && previous_source.is_none()
                && block.provider_source.is_some()
            {
                let old_key = (block.request_id.clone(), previous.0, previous.1, previous.2);
                let new_key = (block.request_id.clone(), item.clone(), kind.into(), part);
                if let Some(prior) = self.reasoning_parts.get(&old_key).cloned() {
                    let merged = match self.reasoning_parts.get(&new_key) {
                        Some(existing) if existing.starts_with(&prior) => existing.clone(),
                        Some(existing) if prior.starts_with(existing) => prior,
                        Some(_) => {
                            return Err(invalid(
                                "native reasoning source conflicts with emitted prefix",
                            ));
                        }
                        None => prior,
                    };
                    self.reasoning_parts.remove(&old_key);
                    self.reasoning_parts.insert(new_key, merged);
                }
            }
            reasoning_coordinate = Some(coordinate);
            if !block.delta.is_empty() {
                self.reasoning_parts
                    .entry((block.request_id.clone(), item.clone(), kind.into(), part))
                    .or_default()
                    .push_str(&block.delta);
            } else {
                let summaries: Vec<(String, usize, String)> = match block.reasoning.as_ref() {
                    Some(ReasoningBlock::Signed { thinking, .. }) => {
                        vec![("thinking".into(), part, thinking.clone())]
                    }
                    Some(ReasoningBlock::Opaque { summary, .. }) => summary
                        .iter()
                        .enumerate()
                        .map(|(index, value)| ("summary".into(), index, value.clone()))
                        .collect(),
                    Some(ReasoningBlock::Redacted { .. }) | None => Vec::new(),
                };
                // 完整signed/opaque是同provider item的重放载体，不是另一段呈现增量。
                // 只从相同来源坐标核完整前缀，绝不跨request或按正文相似度去重。
                for (kind, index, full) in summaries {
                    let key = (block.request_id.clone(), item.clone(), kind, index);
                    let previous = self.reasoning_parts.entry(key).or_default();
                    let missing = full.strip_prefix(previous.as_str()).ok_or_else(|| {
                        invalid("reasoning payload disagrees with same-item emitted deltas")
                    })?;
                    text.push_str(missing);
                    *previous = full;
                }
            }
        }
        let state = self.blocks.entry(canonical_key).or_default();
        if let Some(source) = &block.provider_source {
            state.source = Some(source.clone());
        }
        if reasoning_coordinate.is_some() {
            state.reasoning_coordinate = reasoning_coordinate;
        }
        let reasoning_started = reasoning && !state.reasoning_seen;
        let reasoning_finished = reasoning && block.complete && !state.complete;
        state.reasoning_seen |= reasoning;
        state.complete |= block.complete;
        state.phase = Some(block.phase);
        if !reasoning {
            state.text.push_str(&block.delta);
        }
        self.observations.insert(observation, block.clone());
        Ok(Some(AssistantTextDelta {
            block: block.clone(),
            text,
            reasoning_started,
            reasoning_finished,
        }))
    }

    /// 保存原ReAct最终通知，不借通知猜request或物理完成，也不再次拼接已呈现正文。
    pub fn confirm_answer(&mut self, answer: &str) -> Result<()> {
        if self.execution_id.is_none() || self.terminal.is_some() {
            return Err(invalid(
                "final confirmation outside active native execution",
            ));
        }
        if self
            .confirmed_answer
            .as_deref()
            .is_some_and(|prior| prior != answer)
        {
            return Err(invalid("conflicting final confirmation"));
        }
        self.confirmed_answer
            .get_or_insert_with(|| answer.to_string());
        Ok(())
    }

    /// 从已验证的原生 Settled 与其 request 坐标补最终显示；只返回已批准正文的尾文。
    /// ACP 等逐事件 sink 可在 Settled 时调用用于显示，这不证明物理 EOF 或 Turn receipt。
    /// 执行 owner 必须继续读同一原 source 到真实 EOF，之后才可调用 `finish`。
    /// final通知没有request_id，因此不能把‘最近Unknown正文’猜作当前最终候选。
    pub fn take_final_presentation(&mut self) -> Result<Option<String>> {
        if self.final_presentation_taken {
            return Ok(None);
        }
        let terminal = self
            .terminal
            .as_ref()
            .filter(|status| status.status == ExecutionStatusKind::Settled)
            .ok_or_else(|| invalid("final presentation requires native settlement"))?;
        let request = terminal
            .request_id
            .as_deref()
            .filter(|request| !request.trim().is_empty())
            .ok_or_else(|| invalid("settled final lacks its native request identity"))?;
        let answer = self
            .confirmed_answer
            .as_deref()
            .ok_or_else(|| invalid("settled execution lacks final notification"))?;
        let explicit_final = self.blocks.iter().any(|((owner, _), state)| {
            owner == request && state.phase == Some(AssistantPhase::FinalAnswer)
        });
        let phase = if explicit_final {
            AssistantPhase::FinalAnswer
        } else {
            AssistantPhase::Unknown
        };
        let mut ordered: Vec<_> = self
            .observations
            .iter()
            .filter_map(|((owner, block, _), observation)| {
                (owner == request
                    && self
                        .blocks
                        .get(&(owner.clone(), block.clone()))
                        .is_some_and(|state| state.phase == Some(phase)))
                .then_some((observation.sequence, observation.delta.as_str()))
            })
            .collect();
        ordered.sort_by_key(|(sequence, _)| *sequence);
        let already_presented: String = ordered.into_iter().map(|(_, text)| text).collect();
        let missing = match answer.strip_prefix(&already_presented) {
            Some(suffix) => suffix,
            // Unknown也可能是同响应普通报告；不同安全最终正文由runtime通知完整提供。
            None if !explicit_final => answer,
            None => {
                return Err(invalid(
                    "confirmed final disagrees with approved final blocks",
                ));
            }
        };
        self.final_presentation_taken = true;
        Ok((!missing.is_empty()).then(|| missing.to_string()))
    }

    /// 只读已验证的原生失败，供调用方按观察顺序保留首因；不代表物理EOF。
    pub fn terminal_failure(&self) -> Option<ReactError> {
        match &self.terminal {
            Some(status) if status.status == ExecutionStatusKind::Cancelled => Some(
                crate::error::AgentError::Cancelled(
                    status
                        .reason
                        .clone()
                        .unwrap_or_else(|| "native execution cancelled".into()),
                )
                .into(),
            ),
            Some(status) if status.status == ExecutionStatusKind::Failed => {
                Some(ReactError::Other(
                    status
                        .reason
                        .clone()
                        .unwrap_or_else(|| "native execution failed".into()),
                ))
            }
            _ => None,
        }
    }

    /// 只能在原source真实EOF后调用；失败、缺终态、缺确认均不能成功。
    pub fn finish(&self) -> Result<String> {
        if let Some(error) = self.terminal_failure() {
            return Err(error);
        }
        match &self.terminal {
            Some(status) if status.status == ExecutionStatusKind::Settled => self
                .confirmed_answer
                .clone()
                .ok_or_else(|| invalid("settled execution omitted its confirmed final payload")),
            _ => Err(invalid("source EOF without native execution settlement")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentPhase;

    fn status(execution: &str, sequence: u64, kind: ExecutionStatusKind) -> ExecutionStatus {
        ExecutionStatus {
            adapter_generated: false,
            execution_id: execution.into(),
            request_id: Some("request".into()),
            sequence,
            phase: AgentPhase::Thinking,
            status: kind,
            reason: None,
        }
    }
    fn block(
        request: &str,
        sequence: u64,
        phase: AssistantPhase,
        text: &str,
    ) -> AssistantMessageBlock {
        AssistantMessageBlock {
            provider_source: None,
            block_id: "same-block".into(),
            request_id: request.into(),
            sequence,
            phase,
            delta: text.into(),
            complete: false,
            provider: None,
            reasoning: None,
        }
    }

    #[test]
    fn observations_are_exactly_once_per_execution_request_and_block() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let first = block("request-1", 0, AssistantPhase::Unknown, "中文");
        assert_eq!(
            output.observe_block(&first)?.map(|delta| delta.text),
            Some("中文".into())
        );
        assert!(output.observe_block(&first)?.is_none());
        assert!(
            output
                .observe_block(&block("request-2", 0, AssistantPhase::Unknown, "再读"))?
                .is_some()
        );
        let mut conflict = first;
        conflict.delta = "冲突".into();
        assert!(output.observe_block(&conflict).is_err());
        Ok(())
    }
    #[test]
    fn reasoning_projection_never_exposes_signature_or_opaque_bytes() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut signed = block("request", 0, AssistantPhase::Reasoning, "");
        signed.reasoning = Some(ReasoningBlock::Signed {
            thinking: "摘要".into(),
            signature: "private-signature".into(),
        });
        let delta = output
            .observe_block(&signed)?
            .ok_or_else(|| invalid("fixture delta missing"))?;
        assert_eq!(delta.text, "摘要");
        assert!(delta.reasoning_started);
        let mut end = signed;
        end.sequence = 1;
        end.complete = true;
        let delta = output
            .observe_block(&end)?
            .ok_or_else(|| invalid("fixture end missing"))?;
        assert!(delta.text.is_empty());
        assert!(delta.reasoning_finished);
        let mut opaque = block("request-2", 0, AssistantPhase::Reasoning, "");
        opaque.reasoning = Some(ReasoningBlock::Opaque {
            provider: "responses".into(),
            id: "native".into(),
            data: "encrypted".into(),
            summary: vec!["公开摘要".into()],
        });
        assert_eq!(
            output.observe_block(&opaque)?.map(|delta| delta.text),
            Some("公开摘要".into())
        );
        Ok(())
    }
    #[test]
    fn block_complete_and_final_confirmation_do_not_settle_execution() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut answer = block("request", 0, AssistantPhase::FinalAnswer, "已批准正文");
        answer.complete = true;
        output.observe_block(&answer)?;
        output.confirm_answer("已批准正文")?;
        assert!(output.finish().is_err());
        Ok(())
    }
    #[test]
    fn final_confirmation_is_not_a_second_text_delta_and_foreign_terminal_never_settles()
    -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.observe_block(&block("request", 0, AssistantPhase::FinalAnswer, "正文"))?;
        output.confirm_answer("正文")?;
        output.confirm_answer("正文")?;
        assert!(
            output
                .observe_status(&status("foreign", 1, ExecutionStatusKind::Settled))
                .is_err()
        );
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Settled))?;
        assert!(output.take_final_presentation()?.is_none());
        assert_eq!(output.finish()?, "正文");
        Ok(())
    }
    #[test]
    fn transformed_safe_confirmation_can_finish_without_exposing_raw_blocks() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.confirm_answer("Guard确认的安全正文")?;
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Settled))?;
        assert_eq!(
            output.take_final_presentation()?,
            Some("Guard确认的安全正文".into())
        );
        assert_eq!(output.finish()?, "Guard确认的安全正文");
        Ok(())
    }
    #[test]
    fn current_cancellation_stays_typed_and_old_wrapper_cannot_supply_native_facts() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        let mut generated = status("execution", 0, ExecutionStatusKind::Started);
        generated.adapter_generated = true;
        assert!(output.observe_status(&generated).is_err());
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Cancelled))?;
        assert!(
            matches!(output.finish(), Err(ReactError::Agent(error)) if matches!(error.as_ref(), crate::error::AgentError::Cancelled(_)))
        );
        Ok(())
    }
    #[test]
    fn anthropic_delta_then_signed_snapshot_does_not_repeat_thinking() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut first = block("request", 0, AssistantPhase::Reasoning, "中文");
        first.provider_source = Some(crate::llm::types::ProviderOutputSource {
            protocol: crate::llm::LlmApiProtocol::Anthropic,
            request_id: "request".into(),
            item_id: "thinking-item".into(),
            output_index: 0,
            content_index: None,
            content_kind: Some("thinking".into()),
            phase: None,
        });
        let mut second = first.clone();
        second.sequence = 1;
        second.delta = "摘要".into();
        let mut done = first.clone();
        done.sequence = 2;
        done.delta.clear();
        done.complete = true;
        done.reasoning = Some(ReasoningBlock::Signed {
            thinking: "中文摘要".into(),
            signature: "native-private-signature".into(),
        });
        let mut seen = String::new();
        for observation in [first, second, done] {
            if let Some(delta) = output.observe_block(&observation)? {
                seen.push_str(&delta.text);
            }
        }
        assert_eq!(seen, "中文摘要");
        Ok(())
    }
    #[test]
    fn responses_summary_delta_then_opaque_parent_snapshot_uses_same_item_identity() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut summary = block(
            "responses-request",
            0,
            AssistantPhase::Reasoning,
            "已显示摘要",
        );
        summary.block_id = "item:summary:0".into();
        summary.provider_source = Some(crate::llm::types::ProviderOutputSource {
            protocol: crate::llm::LlmApiProtocol::Responses,
            request_id: "responses-request".into(),
            item_id: "item".into(),
            output_index: 0,
            content_index: Some(0),
            content_kind: Some("summary_text".into()),
            phase: None,
        });
        let mut done = summary.clone();
        done.block_id = "item".into();
        done.sequence = 1;
        done.delta.clear();
        done.complete = true;
        if let Some(source) = done.provider_source.as_mut() {
            source.content_index = None;
            source.content_kind = Some("reasoning".into());
        }
        done.reasoning = Some(ReasoningBlock::Opaque {
            provider: "responses".into(),
            id: "item".into(),
            data: "encrypted-never-display".into(),
            summary: vec!["已显示摘要".into()],
        });
        assert_eq!(
            output.observe_block(&summary)?.map(|delta| delta.text),
            Some("已显示摘要".into())
        );
        assert!(
            output
                .observe_block(&done)?
                .is_some_and(|delta| delta.text.is_empty())
        );
        Ok(())
    }

    #[test]
    fn late_native_source_keeps_canonical_reasoning_prefix_without_repetition() -> Result<()> {
        for opaque in [false, true] {
            let mut output = AgentOutputProjection::default();
            output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
            let mut first = block("canonical-request", 0, AssistantPhase::Reasoning, "已显示");
            first.block_id = "stable-canonical-block".into();
            assert_eq!(
                output.observe_block(&first)?.map(|delta| delta.text),
                Some("已显示".into())
            );
            let mut done = first.clone();
            done.sequence = 1;
            done.delta.clear();
            done.complete = true;
            done.provider_source = Some(crate::llm::types::ProviderOutputSource {
                protocol: if opaque {
                    crate::llm::LlmApiProtocol::Responses
                } else {
                    crate::llm::LlmApiProtocol::Anthropic
                },
                request_id: "actual-native-response".into(),
                item_id: "actual-native-item".into(),
                output_index: 5,
                content_index: opaque.then_some(0),
                content_kind: Some(if opaque { "summary_text" } else { "thinking" }.into()),
                phase: None,
            });
            done.reasoning = Some(if opaque {
                ReasoningBlock::Opaque {
                    provider: "responses".into(),
                    id: "actual-native-item".into(),
                    data: "never-display-encrypted".into(),
                    summary: vec!["已显示新增".into()],
                }
            } else {
                ReasoningBlock::Signed {
                    thinking: "已显示新增".into(),
                    signature: "never-display-signature".into(),
                }
            });
            assert_eq!(
                output.observe_block(&done)?.map(|delta| delta.text),
                Some("新增".into()),
                "late provenance must retain the prefix of the same canonical block"
            );
            let mut foreign = done;
            foreign.request_id = "other-canonical-request".into();
            foreign.sequence = 2;
            assert_eq!(
                output.observe_block(&foreign)?.map(|delta| delta.text),
                Some("已显示新增".into()),
                "another canonical request has no emitted prefix to reuse"
            );
        }
        Ok(())
    }

    #[test]
    fn late_native_source_does_not_move_another_blocks_same_named_native_prefix() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut local = block("canonical-request", 0, AssistantPhase::Reasoning, "甲");
        local.block_id = "same-name".into();
        output.observe_block(&local)?;
        let mut other = local.clone();
        other.block_id = "other-block".into();
        other.sequence = 1;
        other.delta = "乙".into();
        other.provider_source = Some(crate::llm::types::ProviderOutputSource {
            protocol: crate::llm::LlmApiProtocol::Anthropic,
            request_id: "actual-request".into(),
            item_id: "same-name".into(),
            output_index: 9,
            content_index: None,
            content_kind: Some("thinking".into()),
            phase: None,
        });
        output.observe_block(&other)?;
        let mut done = local;
        done.sequence = 2;
        done.delta.clear();
        done.complete = true;
        let mut source = other
            .provider_source
            .clone()
            .ok_or_else(|| invalid("fixture source missing"))?;
        source.item_id = "actual-local-item".into();
        source.output_index = 5;
        done.provider_source = Some(source);
        done.reasoning = Some(ReasoningBlock::Signed {
            thinking: "甲新增".into(),
            signature: "private-signature".into(),
        });
        assert_eq!(
            output.observe_block(&done)?.map(|delta| delta.text),
            Some("新增".into())
        );
        let mut changed = done;
        changed.sequence = 3;
        if let Some(source) = changed.provider_source.as_mut() {
            source.request_id = "foreign-native-request".into();
        }
        assert!(output.observe_block(&changed).is_err());
        Ok(())
    }
    #[test]
    fn confirmed_safe_final_uses_terminal_request_not_previous_unknown_report() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.observe_block(&block(
            "prior-continue-request",
            0,
            AssistantPhase::Unknown,
            "相同文字",
        ))?;
        output.confirm_answer("相同文字")?;
        let mut terminal = status("execution", 1, ExecutionStatusKind::Settled);
        terminal.request_id = Some("final-request-with-no-published-block".into());
        output.observe_status(&terminal)?;
        assert_eq!(output.take_final_presentation()?, Some("相同文字".into()));
        Ok(())
    }

    #[test]
    fn runtime_final_only_supplies_the_missing_approved_prefix_suffix() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.observe_block(&block("request", 0, AssistantPhase::FinalAnswer, "安全"))?;
        output.confirm_answer("安全答案")?;
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Settled))?;
        assert_eq!(output.take_final_presentation()?, Some("答案".into()));
        Ok(())
    }
    #[test]
    fn same_request_unknown_progress_cannot_hide_a_different_guard_safe_final() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        output.observe_block(&block("request", 0, AssistantPhase::Unknown, "进度"))?;
        output.confirm_answer("Guard安全答案")?;
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Settled))?;
        assert_eq!(
            output.take_final_presentation()?,
            Some("Guard安全答案".into())
        );
        assert!(output.take_final_presentation()?.is_none());
        Ok(())
    }
    #[test]
    fn interleaved_blocks_preserve_global_observation_sequence() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?;
        let mut a = block("request", 0, AssistantPhase::FinalAnswer, "a");
        a.block_id = "A".into();
        let mut b = a.clone();
        b.block_id = "B".into();
        b.sequence = 1;
        b.delta = "b".into();
        let mut c = a.clone();
        c.sequence = 2;
        c.delta = "c".into();
        for delta in [a, b, c] {
            output.observe_block(&delta)?;
        }
        output.confirm_answer("abc")?;
        output.observe_status(&status("execution", 1, ExecutionStatusKind::Settled))?;
        assert!(output.take_final_presentation()?.is_none());
        assert_eq!(output.finish()?, "abc");
        Ok(())
    }
    #[test]
    fn exact_terminal_replay_is_idempotent_without_new_settlement_qualification() -> Result<()> {
        let mut output = AgentOutputProjection::default();
        assert!(output.observe_status(&status("execution", 0, ExecutionStatusKind::Started))?);
        output.confirm_answer("答案")?;
        let terminal = status("execution", 1, ExecutionStatusKind::Settled);
        assert!(output.observe_status(&terminal)?);
        assert!(!output.observe_status(&terminal)?);
        Ok(())
    }
}
