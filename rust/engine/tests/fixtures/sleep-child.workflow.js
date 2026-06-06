import { sleep } from "workflow:extra";

export const meta = { name: "sleep-child", description: "sleep child" };

await sleep(5);

export default { childSlept: true, value: args.value };
