import pino from "pino";

export const logger =
  process.env.NODE_ENV === "test"
    ? pino({ level: "silent" })
    : pino({
        level: process.env.LOG_LEVEL?.trim() || "info",
        redact: {
          paths: [
            "*.authorization",
            "*.cookie",
            '*["x-service-secret"]',
            "*.accessToken",
            "*.refreshToken",
            "*.idToken",
            "*.token",
          ],
          censor: "[redacted]",
        },
        ...(process.env.NODE_ENV === "production"
          ? {
              formatters: { level: (label: string) => ({ severity: label.toUpperCase() }) },
              messageKey: "message",
            }
          : {
              transport: {
                target: "pino-pretty",
                options: { colorize: true, ignore: "pid,hostname" },
              },
            }),
      });

