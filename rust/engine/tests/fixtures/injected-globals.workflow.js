export const meta = {
  name: "injected-globals",
  description: "Exercise injected workflow globals",
  phases: [{ title: "Research", detail: "Run research agents" }],
};

export default async function workflow() {
  phase("Research");
  log("received", args);

  const [first, second] = await parallel([
    () => agent(
      `first: ${args["my-arg1"]}`,
      { phase: "Research" }
    ),
    () => agent(
      `second: ${args["my-arg2"]}`,
      { phase: "Research" }
    ),
  ]);

  return { first, second, args };
}
