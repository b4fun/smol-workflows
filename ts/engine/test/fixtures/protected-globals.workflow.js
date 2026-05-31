export default async function workflow(input, ctx) {
  const blocked = [];

  try {
    args["my-arg1"] = "mutated-global-args";
  } catch {
    blocked.push("global-args-set");
  }

  try {
    input["my-arg1"] = "mutated-input";
  } catch {
    blocked.push("input-set");
  }

  try {
    ctx.args["my-arg1"] = "mutated-ctx-args";
  } catch {
    blocked.push("ctx-args-set");
  }

  try {
    args.nested.value = "mutated-nested";
  } catch {
    blocked.push("nested-args-set");
  }

  try {
    agent.extra = "mutated-agent";
  } catch {
    blocked.push("agent-property-set");
  }

  try {
    Object.defineProperty(parallel, "extra", { value: "mutated-parallel" });
  } catch {
    blocked.push("parallel-define-property");
  }

  try {
    pipeline.extra = "mutated-pipeline";
  } catch {
    blocked.push("pipeline-property-set");
  }

  try {
    globalThis.agent = async () => "mutated-global-agent";
  } catch {
    blocked.push("global-agent-reassign");
  }

  return {
    blocked,
    arg: args["my-arg1"],
    inputArg: input["my-arg1"],
    ctxArg: ctx.args["my-arg1"],
    nested: args.nested.value,
    agentExtra: agent.extra ?? null,
    parallelExtra: parallel.extra ?? null,
    pipelineExtra: pipeline.extra ?? null,
    agentResult: await agent(`value: ${args["my-arg1"]}`),
  };
}
