import type { SandboxFn } from "./sandbox.js";

function unsupportedHostModule(): never {
  throw new Error(
    "workflow:sandbox is a smol-workflows host-provided virtual module and cannot be used outside the workflow runtime",
  );
}

/** Advanced: create a reusable workflow-owned sandbox session. */
export const open: SandboxFn["open"] = async () => unsupportedHostModule();

/** Advanced: create a scoped reusable sandbox session. */
export const withSandbox: SandboxFn["with"] = async () => unsupportedHostModule();

const sandbox: SandboxFn = {
  open,
  with: withSandbox,
};

export default sandbox;
