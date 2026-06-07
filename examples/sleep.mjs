import { sleep } from "workflow:extra";

/** @type {import('@smol-workflows/sdk').WorkflowMetadata} */
export const meta = {
  name: "sleep",
  description: "Pause briefly before continuing",
};

log("sleeping for 3 seconds");
await sleep(3000);
log("done sleeping");

export default { slept: true };
