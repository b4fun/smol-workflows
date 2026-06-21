declare module "workflow:sandbox" {
  export const exec: import("./index.js").SandboxFn["exec"];
  export const open: import("./index.js").SandboxFn["open"];
  export const withSandbox: import("./index.js").SandboxFn["with"];
  const sandbox: import("./index.js").SandboxFn;
  export default sandbox;
}
