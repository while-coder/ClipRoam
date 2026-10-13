import { native } from "./native";

export const isNative = native.kind !== "web";
export const previewSource = (path: string): Promise<string> => native.previewSource(path);
