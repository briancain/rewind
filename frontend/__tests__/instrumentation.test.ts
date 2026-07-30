import { onRequestError } from "../instrumentation";

describe("onRequestError", () => {
  it("emits a flat JSON line with top-level status 500 (what the CloudWatch filter matches)", () => {
    const spy = jest.spyOn(console, "error").mockImplementation(() => {});

    onRequestError(
      new Error("boom"),
      { path: "/watch/abc", method: "GET", headers: {} } as never,
      { routerKind: "App Router", routePath: "/watch/[id]", routeType: "render" } as never,
    );

    expect(spy).toHaveBeenCalledTimes(1);
    const logged = JSON.parse(spy.mock.calls[0][0] as string);
    expect(logged.status).toBe(500);
    expect(logged.path).toBe("/watch/abc");
    expect(logged.method).toBe("GET");
    expect(logged.message).toBe("boom");
    expect(logged.logger).toBe("frontend");

    spy.mockRestore();
  });
});
