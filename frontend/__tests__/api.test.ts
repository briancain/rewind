import { serviceUrl } from "@/lib/api";

describe("api helper", () => {
  it("constructs service URLs correctly", () => {
    expect(serviceUrl("identity", "/me")).toBe("http://localhost:8080/me");
    expect(serviceUrl("catalog", "/videos/feed")).toBe("http://localhost:8081/videos/feed");
    expect(serviceUrl("search", "/search?q=test")).toBe("http://localhost:8085/search?q=test");
  });
});
