import { sleep } from "workflow:extra";

export const meta = { name: "sleep-parent", description: "sleep parent" };

await sleep(5);
const child = await workflow({ scriptPath: "./sleep-child.workflow.js" }, { value: "from-parent" });

export default { parentSlept: true, child };
