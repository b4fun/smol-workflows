import type { WorkflowExtra } from "./index.js";

function unsupportedHostModule(): never {
  throw new Error(
    "workflow:extra is a smol-workflows host-provided virtual module and cannot be used outside the workflow runtime",
  );
}

/** Pause workflow execution for at least `ms` milliseconds. */
export const sleep: WorkflowExtra["sleep"] = async () => unsupportedHostModule();

const extra: WorkflowExtra = {
  sleep,
};

export default extra;
