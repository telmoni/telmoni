// What `import "server-only"` resolves to under vitest. Next implements the
// specifier in its compiler — a "use client" importer is a build error and a
// server importer is a no-op — so there is no package for vitest to find. Every
// suite runs server code in a plain Node or jsdom process, which is the no-op
// case.
export {};
