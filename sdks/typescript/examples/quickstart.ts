const sdkModuleUrl = new URL("../../dist/index.js", import.meta.url);
const { EchoAgentClient } = (await import(sdkModuleUrl.href)) as typeof import("../src/index.js");

const hostCommand = process.env.ECHO_AGENT_SDK_HOST;
const hostConfig = process.env.ECHO_AGENT_SDK_CONFIG;
if (!hostCommand || !hostConfig) {
  throw new Error("ECHO_AGENT_SDK_HOST and ECHO_AGENT_SDK_CONFIG are required");
}

const sdk = await EchoAgentClient.spawn({
  hostCommand,
  args: ["--config", hostConfig],
});
try {
  const agent = await sdk.createAgent();
  const session = await agent.createSession();
  try {
    const name = await session.invoke<string>("echo_core::agent::Agent::name");
    console.log(name);
  } finally {
    await session.close();
    await agent.close();
  }
} finally {
  await sdk.close();
}
