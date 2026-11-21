export function helper(x: number): number { return x + 1; }
export function target(x: number): number { return helper(x); }
export function test_target(): number { return target(1); }
