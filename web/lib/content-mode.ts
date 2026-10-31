import { ContentMode } from "@/lib/types/enums";

/** The word the console gives each content mode, wherever one is shown. */
export const CONTENT_MODE_LABEL: Record<ContentMode, string> = {
  [ContentMode.Off]: "Off",
  [ContentMode.On]: "On",
};

/** What each mode keeps, in a sentence. */
export const CONTENT_MODE_MEANS: Record<ContentMode, string> = {
  [ContentMode.Off]:
    "Content is dropped as each span arrives, and the span is kept without it.",
  [ContentMode.On]:
    "Content is kept as sent, sealed under the project's own key in Postgres alone, for as long as its span, wherever the code asks the SDK to send it.",
};
