import { sleep } from "workflow:extra";

export const meta = { name: "sleep", description: "sleep" };

await sleep(5);

export default { slept: true, result: await agent("after sleep") };
