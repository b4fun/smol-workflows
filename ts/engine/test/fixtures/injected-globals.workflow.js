export default async function workflow() {
  phase("Research", { metadata: { source: "test" } });
  log("received", args);

  const [first, second] = await parallel([
    () => agent(
      `first: ${args["my-arg1"]}`,
      { key: "first", phase: "Research" }
    ),
    () => agent(
      `second: ${args["my-arg2"]}`,
      { key: "second", phase: "Research" }
    ),
  ]);

  return { first, second, args };
}
