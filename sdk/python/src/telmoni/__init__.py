"""The Python SDK for Telmoni, privacy-first, open-source monitoring for AI
agents: an agent's traces, steps, model calls, tool calls and retrievals as
OpenTelemetry spans, sent to Telmoni and nowhere else."""

from ._version import __version__
from .client import Telmoni
from .guard import CapExceededError
from .models import (
    Config,
    ModelCallRecord,
    Price,
    Secret,
    StepRecord,
    ToolCallRecord,
    TraceRecord,
    Usage,
    WarningInfo,
    WarningKind,
)
from .processor import Batching, QueueFullError, ShutdownError
from .traces import ModelCall, Retrieval, Step, ToolCall, Trace, current_trace

__all__ = [
    "Batching",
    "CapExceededError",
    "Config",
    "ModelCall",
    "ModelCallRecord",
    "Price",
    "QueueFullError",
    "Retrieval",
    "Secret",
    "ShutdownError",
    "Step",
    "StepRecord",
    "Telmoni",
    "ToolCall",
    "ToolCallRecord",
    "Trace",
    "TraceRecord",
    "Usage",
    "WarningInfo",
    "WarningKind",
    "__version__",
    "current_trace",
]
