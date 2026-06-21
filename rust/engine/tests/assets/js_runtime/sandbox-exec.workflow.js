import { exec } from "workflow:sandbox";

export const meta = { name: "sandbox-exec", description: "sandbox exec" };

const globalSandboxExecType = typeof SW.sandbox.exec;
const genericSandboxType = typeof sandbox;
const value = await exec("exe-dev/default", {
  command: "sh",
  args: ["-lc", "pwd"],
  cwd: "/workspace",
  env: { EXAMPLE: "1" },
  stdin: "hello",
});

export default { value, globalSandboxExecType, genericSandboxType };
