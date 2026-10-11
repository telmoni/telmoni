import { describe, expect, it } from "vitest";

import {
  conversationHref,
  conversationsHref,
  currentConversationSort,
  nextConversationSort,
  parseConversationQuery,
} from "./conversations";

describe("the conversations list's URL", () => {
  it("reads the sort and the window, and drops what the server would refuse", () => {
    expect(
      parseConversationQuery({ sort: "conversation", dir: "asc", days: "90", limit: "200" }),
    ).toEqual({
      sort: "conversation",
      dir: "asc",
      days: 90,
      limit: 200,
    });
    expect(parseConversationQuery({ sort: "agent", dir: "up", days: "361", limit: "0" })).toEqual({});
    // The word the page no longer answers to is dropped like any other.
    expect(parseConversationQuery({ sort: "session" })).toEqual({});
  });

  it("writes the list's path with one thing changed, and a conversation's own", () => {
    expect(conversationsHref("acme", "web", {})).toBe("/acme/web/conversations");
    expect(conversationsHref("acme", "web", { sort: "user", days: 30 }, { dir: "desc" })).toBe(
      "/acme/web/conversations?sort=user&days=30&dir=desc",
    );
    expect(conversationHref("acme", "web", "conv-1")).toBe("/acme/web/conversations/conv-1");
    // An id of the application's choosing is encoded whole, slash and all.
    expect(conversationHref("acme", "web", "chat/42 b")).toBe(
      "/acme/web/conversations/chat%2F42%20b",
    );
    expect(conversationHref("acme", "web", "100%", { days: 30, limit: 500 })).toBe(
      "/acme/web/conversations/100%25?days=30&limit=500",
    );
  });

  it("sorts the conversation last heard from first by default, and each heading its own way", () => {
    expect(currentConversationSort({})).toEqual({ sort: "last_seen", dir: "desc" });
    expect(currentConversationSort({ sort: "conversation" })).toEqual({
      sort: "conversation",
      dir: "asc",
    });
    expect(nextConversationSort({}, "last_seen")).toEqual({ sort: "last_seen", dir: "asc" });
    expect(nextConversationSort({}, "user")).toEqual({ sort: "user", dir: "asc" });
    expect(nextConversationSort({ sort: "tokens" }, "tokens")).toEqual({
      sort: "tokens",
      dir: "asc",
    });
  });
});
