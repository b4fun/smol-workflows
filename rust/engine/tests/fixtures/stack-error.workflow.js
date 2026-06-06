export const meta = {
  name: 'stack-error',
  description: 'Exercise stack traces for module evaluation errors',
};

function fail() {
  throw new Error('boom from module');
}

fail();

export default 'unreachable';
