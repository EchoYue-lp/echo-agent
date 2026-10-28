//! 当前原生 Agent 流的纯输出归并，不拥有执行、取消、持久化或物理结算。
//!
//! 每个实例只观察一条原 producer 流。调用方必须继续读取到真实 EOF，之后才调用
//! `finish`；块 complete、FinalAnswer 确认和 foreign terminal 都不能替代该读取。

use super::{AssistantMessageBlock, AssistantPhase, ExecutionStatus, ExecutionStatusKind};
use crate::error::{ReactError, Result};
use crate::llm::types::ReasoningBlock;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct AssistantTextDelta {
    pub block: AssistantMessageBlock,
    pub text: String,
    pub reasoning_started: bool,
    pub reasoning_finished: bool,
}

#[derive(Default)]
struct BlockProjection {
    reasoning_seen: bool,
    complete: bool,
    phase: Option<AssistantPhase>,
    text: String,
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
    reasoning_parts: BTreeMap<(String, String, String, usize), String>,
    terminal: Option<ExecutionStatus>,
}

fn invalid(message: &str) -> ReactError {
    ReactError::Other(format!("invalid canonical Agent output: {message}"))
}

impl AgentOutputProjection {
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
        if reasoning {
            let item = block
                .provider_source
                .as_ref()
                .map(|source| source.item_id.clone())
                .unwrap_or_else(|| match block.reasoning.as_ref() {
                    Some(ReasoningBlock::Opaque { id, .. }) => id.clone(),
                    _ => block.block_id.clone(),
                });
            let kind = if block
                .provider_source
                .as_ref()
                .and_then(|source| source.content_kind.as_deref())
                == Some("summary_text")
            {
                "summary"
            } else {
                "thinking"
            };
            let part = block
                .provider_source
                .as_ref()
                .and_then(|source| source.content_index)
                .map(|index| index as usize)
                .unwrap_or(0);
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
        let key = (block.request_id.clone(), block.block_id.clone());
        let state = self.blocks.entry(key).or_default();
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

    /// 调用方读完原source EOF后，从真实runtime terminal的request坐标补最终呈现。
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
