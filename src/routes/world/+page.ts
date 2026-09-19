// The world is a canvas, a GL context and an IPC channel: none of that exists
// on a server, and prerendering the route would only bake an empty <canvas>
// into the bundle.
export const ssr = false;
export const prerender = false;
