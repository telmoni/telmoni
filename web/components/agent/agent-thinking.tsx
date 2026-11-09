export function AgentThinking() {
  return (
    <div className="my-2 flex h-6 items-center">
      <div className="agent-thinking-wrapper" aria-hidden="true">
        <div className="agent-dot agent-dot-1" />
        <div className="agent-dot agent-dot-2" />
        <div className="agent-dot agent-dot-3" />
      </div>
      <span className="sr-only">Thinking…</span>
    </div>
  );
}
