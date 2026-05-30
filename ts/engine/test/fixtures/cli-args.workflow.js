export default async function workflow() {
  return {
    args,
    result: await agent(`hello ${args["my-arg1"]}`),
  };
}
