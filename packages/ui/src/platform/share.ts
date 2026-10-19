import type { Unlisten } from "./native";

export type SharedContent = { id: string; text?: string; html?: string; paths: string[]; files?: File[] };

/** Platform inbox: receive notifications, read pending content and acknowledge successful imports. */
export interface ShareReceiver {
  isSupported(): Promise<boolean>;
  listen(handler: () => void): Promise<Unlisten>;
  pending(scope: string): Promise<SharedContent[]>;
  acknowledge(id: string): Promise<void>;
}

/** Desktop, Web and extensions without a share entry point use this default. */
export const noShareReceiver: ShareReceiver = {
  async isSupported() { return false; },
  async listen() { return () => {}; },
  async pending() { return []; },
  async acknowledge() {},
};

export let shareReceiver: ShareReceiver = noShareReceiver;
export function configureShareReceiver(receiver: ShareReceiver): void { shareReceiver = receiver; }
