import asyncio
import os

import pytest

from echo_agent_sdk import EchoAgentClient


@pytest.mark.asyncio
async def test_source_sdk_reaches_real_host() -> None:
    host = os.environ.get("ECHO_AGENT_SDK_HOST")
    config = os.environ.get("ECHO_AGENT_SDK_CONFIG")
    if not host or not config:
        pytest.skip("language SDK host smoke is enabled by check-language-sdks.sh")
    async with await EchoAgentClient.spawn(host, "--config", config) as sdk:

        async def smoke_handler(_call, _cancel):
            return {
                "outcome": "result",
                "result": {
                    "operation": "llm_chat",
                    "value": {
                        "message": {
                            "role": "assistant",
                            "content": {"kind": "string", "value": "python-smoke"},
                        },
                        "finish_reason": "stop",
                        "raw": {"kind": "map", "value": []},
                    },
                },
            }

        registration = await sdk.register_llm_client(
            "python-smoke-llm",
            {
                "kind": "llm_client",
                "descriptor_version": 1,
                "model_name": "python-smoke-model",
                "supports_streaming": False,
            },
            smoke_handler,
        )
        agent = await sdk.create_agent()
        session = await agent.create_session()
        update = asyncio.create_task(anext(session.updates()))
        assert await session.invoke("echo_core::agent::Agent::name") == "echo-agent"
        await session.invoke("memory.store.put", [["sdk"], "python", "ok"])
        stored = await session.invoke("memory.store.get", [["sdk"], "python"])
        assert stored["value"] == "ok"
        telemetry = await sdk.call("telemetry.status", None)
        assert isinstance(telemetry["initialized"], bool)
        prompt = await session.prompt("hello")
        assert prompt.stop_reason == "end_turn"
        first_update = await asyncio.wait_for(update, timeout=10)
        assert first_update["sessionId"] == session.acp_session_id
        assert isinstance(first_update["update"], dict)
        run = await session.start_run("run smoke")
        event = asyncio.create_task(anext(run.events))
        first_event = await asyncio.wait_for(event, timeout=10)
        assert first_event["stream"]["id"] == run.stream.id
        assert first_event["envelope"]["stream_id"] == run.stream.id
        waited = await run.wait()
        assert waited["settled"] is True
        run_state = await run.get()
        assert run_state["status"] == "completed"
        assert await run.status() == "completed"
        assert await run.outcome_status() == "completed"
        usage = await run.usage()
        assert isinstance(usage["duration_ms"], str)
        assert usage["duration_ms"].isdigit()
        assert usage["tokens_used"] is None or isinstance(usage["tokens_used"], str)
        assert usage["iterations"] is None or isinstance(usage["iterations"], str)
        await run.close()
        await registration.close()
        await session.close()
        await agent.close()
