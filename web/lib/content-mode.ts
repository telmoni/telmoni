import { ContentMode } from "@/lib/types/enums";

/** The word the console gives each content mode, wherever one is shown. */
export const CONTENT_MODE_LABEL: Record<ContentMode, string> = {
  [ContentMode.Off]: "Off",
  [ContentMode.On]: "On",
};
