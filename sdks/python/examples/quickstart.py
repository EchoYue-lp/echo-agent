from __future__ import annotations

import asyncio
import os

from echo_agent_sdk import EchoAgentClient


async def main() -> None:
    host = os.environ.get("ECHO_AGENT_SDK_HOST")
    config = os.environ.get("ECHO_AGENT_SDK_CONFIG")
    if not host or not config:
        raise RuntimeError("ECHO_AGENT_SDK_HOST and ECHO_AGENT_SDK_CONFIG are required")
    async with await EchoAgentClient.spawn(host, "--config", config) as sdk:
        agent = await sdk.create_agent()
        try:
            session = await agent.create_session()
            try:
                print(await session.invoke("echo_core::agent::Agent::name"))
            finally:
                await session.close()
        finally:
            await agent.close()


if __name__ == "__main__":
    asyncio.run(main())
