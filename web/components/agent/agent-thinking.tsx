export function AgentThinking() {
  return (
    <div className="flex items-center h-6 my-2 bg-transparent ring-0 shadow-none">
      <div className="agent-thinking-wrapper bg-transparent" aria-hidden="true">
        <div className="agent-dot agent-dot-1" />
        <div className="agent-dot agent-dot-2" />
        <div className="agent-dot agent-dot-3" />
      </div>
      <span className="sr-only">Thinking…</span>
    </div>
  );
}
