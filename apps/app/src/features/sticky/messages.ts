import type { ClipboardEntry } from "@cliproam/protocol";

export type StickyLayout = { x: number; y: number; alwaysOnTop: boolean };
export type StickyPayload = { entry: ClipboardEntry; imageSource?: string; imageError?: string };
export type StickyAction = { label: string; action: "ready" | "copy" | "save" | "unpin" | "image" | "layout"; layout?: StickyLayout };
export type StickyInputMode = { interactive: boolean; altSupported: boolean; manualEdit: boolean };
export const STICKY_ACTION = "cliproam://sticky-action";
export const STICKY_CONTENT = "cliproam://sticky-content";
export const STICKY_RESULT = "cliproam://sticky-result";
export const STICKY_EDIT = "cliproam://sticky-edit";
export const STICKY_INPUT = "cliproam://sticky-input";
export const STICKY_OPACITY = "cliproam://sticky-opacity";
export const STICKY_OPACITY_PREVIEW = "cliproam://sticky-opacity-preview";
