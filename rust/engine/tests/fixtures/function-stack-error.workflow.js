export const meta = {
  name: 'function-stack-error',
  description: 'Exercise stack traces for default function errors',
};

function fail() {
  throw new Error('boom from function');
}

export default async function workflow() {
  fail();
}
