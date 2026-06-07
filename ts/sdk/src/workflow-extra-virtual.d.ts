declare module "workflow:extra" {
  export const sleep: import("./index.js").WorkflowExtra["sleep"];
  const extra: import("./index.js").WorkflowExtra;
  export default extra;
}
