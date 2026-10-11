import { ContentMode } from "@/lib/types/enums";

/** The word the console gives each content mode, wherever one is shown. */
export const CONTENT_MODE_LABEL: Record<ContentMode, string> = {
  [ContentMode.Off]: "Off",
  [ContentMode.On]: "On",
  [ContentMode.Sealed]: "Sealed",
};

/** What each mode keeps, in a sentence. */
export const CONTENT_MODE_MEANS: Record<ContentMode, string> = {
  [ContentMode.Off]:
    "Content is dropped as each span arrives, and the span is kept without it.",
  [ContentMode.On]:
    "Content is kept as it arrives, in Postgres alone, and every read of it is audited.",
  [ContentMode.Sealed]:
    "Only content the SDK encrypted under your key is kept; the key never reaches Telmoni.",
};
