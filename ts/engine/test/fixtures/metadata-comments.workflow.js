export const meta = {
  // comments should not affect AST extraction
  "name": "quoted-keys",
  description: "description with { braces } in a string",
  phases: [
    {
      title: "Research",
      detail: "detail with // not a comment and /* not a comment */",
      provider: "debug",
    },
  ],
};
