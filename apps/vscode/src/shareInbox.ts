import type { Memento } from "vscode";
import { randomUUID } from "node:crypto";

type PendingSelection = { id: string; text: string; scope?: string };
const STORAGE_KEY = "cliproam.pending-selections";

/** Persist editor selections outside the Webview until the account confirms import. */
export class VscodeShareInbox {
  #work: Promise<unknown> = Promise.resolve();
  constructor(private readonly storage: Memento) {}

  private update(transform: (pending: PendingSelection[]) => PendingSelection[]): Promise<void> {
    const work = this.#work.then(() => this.storage.update(STORAGE_KEY,
      transform(this.storage.get<PendingSelection[]>(STORAGE_KEY, []))));
    this.#work = work.catch(() => undefined);
    return work;
  }
  enqueue(text: string): Promise<void> {
    return this.update((pending) => [...pending, { id: randomUUID(), text }]);
  }
  async pending(scope: string): Promise<{ id: string; text: string; images: string[]; paths: string[] }[]> {
    let selected: PendingSelection[] = [];
    await this.update((pending) => {
      const claimed = pending.map((item) => item.scope ? item : { ...item, scope });
      selected = claimed.filter((item) => item.scope === scope);
      return claimed;
    });
    return selected.map(({ id, text }) => ({ id, text, images: [], paths: [] }));
  }
  acknowledge(id: string): Promise<void> { return this.update((pending) => pending.filter((item) => item.id !== id)); }
}
