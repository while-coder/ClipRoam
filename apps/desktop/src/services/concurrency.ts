/**
 * A folder can hold thousands of files, so transfers run through a fixed-size
 * worker pool instead of one promise per file.
 */
export const TRANSFER_CONCURRENCY = 4;

/**
 * Runs `worker` over `items` with at most `limit` in flight. Unlike
 * `Promise.allSettled` over the whole list, memory and socket pressure stay
 * bounded no matter how many items there are. Results keep the input order.
 */
export async function mapWithConcurrency<T, R>(
  items: readonly T[],
  limit: number,
  worker: (item: T, index: number) => Promise<R>,
): Promise<PromiseSettledResult<R>[]> {
  const results = new Array<PromiseSettledResult<R>>(items.length);
  let next = 0;

  const runner = async (): Promise<void> => {
    for (let index = next++; index < items.length; index = next++) {
      try {
        results[index] = { status: "fulfilled", value: await worker(items[index]!, index) };
      } catch (reason) {
        results[index] = { status: "rejected", reason };
      }
    }
  };

  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, runner));
  return results;
}
