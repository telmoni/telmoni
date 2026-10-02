
export function AgentThinking() {
  return (
    <div className="flex items-center h-6 my-2">
      <div className="agent-thinking-wrapper" aria-hidden="true">
        <div className="agent-dot agent-dot-1" />
        <div className="agent-dot agent-dot-2" />
        <div className="agent-dot agent-dot-3" />
      </div>
      <style>{`
        .agent-thinking-wrapper {
          position: relative;
          width: 20px;
          height: 20px;
          animation: agent-spin 2.5s infinite cubic-bezier(0.4, 0, 0.2, 1);
        }
        
        .agent-dot {
          position: absolute;
          top: 50%;
          left: 50%;
          width: 4px;
          height: 4px;
          margin-top: -2px;
          margin-left: -2px;
          background-color: var(--foreground);
          border-radius: 50%;
        }

        .agent-dot-1 {
          animation: agent-dot1 2.5s infinite cubic-bezier(0.4, 0, 0.2, 1);
        }
        .agent-dot-2 {
          animation: agent-dot2 2.5s infinite cubic-bezier(0.4, 0, 0.2, 1);
        }
        .agent-dot-3 {
          animation: agent-dot3 2.5s infinite cubic-bezier(0.4, 0, 0.2, 1);
        }

        @keyframes agent-spin {
          0% { transform: rotate(0deg); }
          50% { transform: rotate(180deg); }
          100% { transform: rotate(360deg); }
        }

        @keyframes agent-dot1 {
          0%, 20%, 100% { transform: translate(-6px, 0); }
          40%, 60% { transform: translate(-5.2px, 3px); }
        }
        @keyframes agent-dot2 {
          0%, 20%, 100% { transform: translate(0px, 0); }
          40%, 60% { transform: translate(0px, -6px); }
        }
        @keyframes agent-dot3 {
          0%, 20%, 100% { transform: translate(6px, 0); }
          40%, 60% { transform: translate(5.2px, 3px); }
        }
      `}</style>
      <span className="sr-only">Thinking…</span>
    </div>
  );
}
