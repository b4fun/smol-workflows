export const meta = {
  name: "cli-args",
  description: "Exercise CLI-provided workflow args",
};

export default async function workflow() {
  return {
    args,
    result: await agent(`hello ${args["my-arg1"]}`),
  };
}
