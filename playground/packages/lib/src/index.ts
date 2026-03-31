import { add } from '@playground/utils';

export function sum(...nums: number[]): number {
  return nums.reduce((acc, n) => add(acc, n), 0);
}
