import { useState } from "react";

const W = 1080, H = 1350;
const SC = 0.34;

function Slide({ children }) {
  return (
    <div style={{ width: W, height: H, background: "#0d1117", display: "flex", flexDirection: "column", justifyContent: "center", padding: "0 100px", position: "relative", fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif", boxSizing: "border-box", transform: `scale(${SC})`, transformOrigin: "top left" }}>
      {children}
    </div>
  );
}

function Tag({ color, children }) {
  return <div style={{ fontSize: 26, fontWeight: 700, color, textTransform: "uppercase", letterSpacing: "0.12em", marginBottom: 50 }}>{children}</div>;
}

function Footer({ right }) {
  return (
    <div style={{ position: "absolute", bottom: 50, left: 100, right: 100, fontSize: 24, color: "#484f58" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span style={{ fontWeight: 600, color: "#8b949e" }}>Rudra Pratap Singh</span>
        <span>{right}</span>
      </div>
      <div style={{ display: "flex", gap: 32, marginTop: 12, alignItems: "center" }}>
        <span style={{ display: "flex", alignItems: "center", gap: 10, color: "#8b949e" }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="#8b949e"><path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z"/></svg>
          <span>@TheNextIdeaGuy</span>
        </span>
        <span style={{ display: "flex", alignItems: "center", gap: 10, color: "#484f58" }}>
          <svg width="24" height="24" viewBox="0 0 24 24" fill="#484f58"><path d="M20.447 20.452h-3.554v-5.569c0-1.328-.027-3.037-1.852-3.037-1.853 0-2.136 1.445-2.136 2.939v5.667H9.351V9h3.414v1.561h.046c.477-.9 1.637-1.85 3.37-1.85 3.601 0 4.267 2.37 4.267 5.455v6.286zM5.337 7.433a2.062 2.062 0 01-2.063-2.065 2.064 2.064 0 112.063 2.065zm1.782 13.019H3.555V9h3.564v11.452zM22.225 0H1.771C.792 0 0 .774 0 1.729v20.542C0 23.227.792 24 1.771 24h20.451C23.2 24 24 23.227 24 22.271V1.729C24 .774 23.2 0 22.222 0h.003z"/></svg>
          <span>rudraptpsingh</span>
        </span>
      </div>
    </div>
  );
}

const S1 = () => (
  <Slide>
    <div style={{ position: "absolute", inset: 0, opacity: 0.04, fontFamily: "monospace", fontSize: 22, lineHeight: "34px", color: "#58a6ff", padding: 50, whiteSpace: "pre-wrap", overflow: "hidden" }}>
      {`CPU: ████████████████ 99.97%\nRAM: ██████████████░ 91.2%\nHEADROOM: insufficient\n\n> process_blame\n  culprit: "claude-code"\n  impact: "critical"\n  fix: "kill stale sessions"\n\nALERT: ram_pressure WARN→CRIT`}
    </div>
    <div style={{ position: "relative", zIndex: 1 }}>
      <div style={{ fontSize: 68, fontWeight: 800, color: "#f0f6fc", lineHeight: 1.2 }}>Your machine is dying.</div>
      <div style={{ fontSize: 68, fontWeight: 800, color: "#f85149", lineHeight: 1.2, marginTop: 30 }}>Your agent is spawning another subprocess.</div>
      <div style={{ marginTop: 80, fontSize: 34, color: "#8b949e" }}>I built the missing feedback loop.</div>
    </div>
    <Footer right={<span style={{ color: "#58a6ff" }}>SWIPE →</span>} />
  </Slide>
);

const S2 = () => (
  <Slide>
    <Tag color="#d2992a">The data</Tag>
    <div style={{ fontSize: 160, fontWeight: 800, color: "#f85149", lineHeight: 1 }}>19%</div>
    <div style={{ fontSize: 160, fontWeight: 800, color: "#f0f6fc", lineHeight: 1, marginTop: 10 }}>slower.</div>
    <div style={{ marginTop: 70, fontSize: 34, color: "#8b949e", lineHeight: 1.7 }}>Devs using AI tools were 19% slower.<br/>They <span style={{ color: "#f0f6fc", fontWeight: 700 }}>thought</span> they were 20% faster.</div>
    <div style={{ marginTop: 60, fontSize: 32, color: "#f0f6fc", fontWeight: 600, borderLeft: "5px solid #d2992a", paddingLeft: 28 }}>The bottleneck moved to hardware.</div>
    <Footer right="2 / 10" />
  </Slide>
);

const S3 = () => (
  <Slide>
    <Tag color="#f85149">The blind spot</Tag>
    <div style={{ fontSize: 50, fontWeight: 700, color: "#f0f6fc", lineHeight: 1.35 }}>Agents can read your machine.</div>
    <div style={{ fontSize: 50, fontWeight: 700, color: "#8b949e", lineHeight: 1.35, marginTop: 14 }}>They've never let it guide a single decision.</div>
    <div style={{ marginTop: 60, display: "flex", flexDirection: "column", gap: 28 }}>
      {["Can shell out to ps aux", "Can parse top output", "Can read /proc/stat"].map((t, i) => (
        <div key={i} style={{ fontSize: 32, color: "#8b949e" }}><span style={{ color: "#3fb950", marginRight: 18 }}>✓</span>{t}</div>
      ))}
    </div>
    <div style={{ marginTop: 44, display: "flex", flexDirection: "column", gap: 28 }}>
      {["None self-regulate", "None adjust parallelism", "None pause at 90% RAM"].map((t, i) => (
        <div key={i} style={{ fontSize: 32, color: "#f85149" }}><span style={{ marginRight: 18 }}>✗</span>{t}</div>
      ))}
    </div>
    <Footer right="3 / 10" />
  </Slide>
);

const S4 = () => (
  <Slide>
    <Tag color="#f85149">Real incident</Tag>
    <div style={{ fontSize: 48, fontWeight: 700, color: "#f0f6fc", lineHeight: 1.35 }}>I wasted 20 minutes debugging tests that weren't broken.</div>
    <div style={{ marginTop: 56, fontSize: 34, color: "#c9d1d9", lineHeight: 1.7 }}>Vibe coding on my M4 MacBook Air.<br/>Fans screaming. Tests flaking.</div>
    <div style={{ marginTop: 56, padding: "36px 40px", background: "rgba(248,81,73,0.05)", borderRadius: 20, border: "2px solid rgba(248,81,73,0.1)" }}>
      <div style={{ fontSize: 24, color: "#f85149", fontWeight: 600, marginBottom: 18 }}>ACTIVITY MONITOR</div>
      <div style={{ color: "#f0f6fc", fontWeight: 700, fontSize: 40 }}>5 Claude processes.</div>
      <div style={{ color: "#f0f6fc", fontWeight: 700, fontSize: 40, marginTop: 6 }}>14GB on a 16GB machine.</div>
    </div>
    <div style={{ marginTop: 56, fontSize: 40, fontWeight: 700, color: "#f85149" }}>My machine was drowning.</div>
    <Footer right="4 / 10" />
  </Slide>
);

const S5 = () => (
  <Slide>
    <Tag color="#f85149">The receipts</Tag>
    <div style={{ fontSize: 42, fontWeight: 700, color: "#f0f6fc", marginBottom: 56 }}>From the Claude Code repo:</div>
    <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
      {[["#24960","Kernel panic. Forced power-off."],["#18859","60GB overnight. Full crash."],["#15487","24 sub-agents. System lockup."],["#33963","OOM. No degradation."]].map(([id,t],i)=>(
        <div key={i} style={{ padding: "24px 28px", background: "rgba(248,81,73,0.04)", borderRadius: 14, border: "2px solid rgba(248,81,73,0.08)", display: "flex", alignItems: "center", gap: 24 }}>
          <span style={{ fontFamily: "monospace", fontSize: 28, color: "#f85149", fontWeight: 700 }}>{id}</span>
          <span style={{ fontSize: 30, color: "#c9d1d9" }}>{t}</span>
        </div>
      ))}
    </div>
    <div style={{ marginTop: 56, fontSize: 28, color: "#8b949e", fontStyle: "italic" }}>The agent kept going while the machine was failing.</div>
    <Footer right="5 / 10" />
  </Slide>
);

const S6 = () => (
  <Slide>
    <Tag color="#58a6ff">The fix</Tag>
    <div style={{ fontSize: 64, fontWeight: 800, color: "#58a6ff" }}>axon</div>
    <div style={{ marginTop: 14, fontSize: 40, color: "#8b949e" }}>Hardware awareness for AI agents.</div>
    <div style={{ marginTop: 70, display: "flex", flexDirection: "column", gap: 38 }}>
      {[["Local MCP server. 7 tools.",false],["Collects state every 2 seconds.",false],["Zero network calls. Ever.",true],["macOS + Linux. Open source.",false]].map(([t,g],i)=>(
        <div key={i} style={{ fontSize: 34, color: g ? "#3fb950" : "#c9d1d9", fontWeight: g ? 700 : 400, display: "flex", alignItems: "center", gap: 24 }}>
          <div style={{ width: 12, height: 12, borderRadius: "50%", background: g ? "#3fb950" : "#58a6ff", flexShrink: 0 }}/>
          {t}
        </div>
      ))}
    </div>
    <div style={{ marginTop: 70, fontSize: 32, color: "#f0f6fc", fontWeight: 600, borderLeft: "5px solid #58a6ff", paddingLeft: 28 }}>Not monitoring. A decision-shaping interface.</div>
    <Footer right="6 / 10" />
  </Slide>
);

const S7 = () => (
  <Slide>
    <Tag color="#58a6ff">Before / after</Tag>
    <div style={{ padding: "44px 40px", background: "rgba(248,81,73,0.04)", borderRadius: 20, border: "2px solid rgba(248,81,73,0.08)" }}>
      <div style={{ fontSize: 24, fontWeight: 700, color: "#f85149", textTransform: "uppercase", letterSpacing: "0.08em" }}>Without axon</div>
      <div style={{ marginTop: 28, fontSize: 32, color: "#8b949e", lineHeight: 1.7 }}>Agent runs ps aux + top + vm_stat<br/>Parses output. Guesses wrong.</div>
      <div style={{ marginTop: 24, fontSize: 40, color: "#f85149", fontWeight: 700 }}>~3,000 tokens.</div>
    </div>
    <div style={{ marginTop: 34, padding: "44px 40px", background: "rgba(63,185,80,0.04)", borderRadius: 20, border: "2px solid rgba(63,185,80,0.08)" }}>
      <div style={{ fontSize: 24, fontWeight: 700, color: "#3fb950", textTransform: "uppercase", letterSpacing: "0.08em" }}>With axon</div>
      <div style={{ marginTop: 28, fontSize: 32, color: "#c9d1d9", lineHeight: 1.7 }}>Agent calls <span style={{ fontFamily: "monospace", color: "#58a6ff" }}>process_blame</span><br/><span style={{ color: "#f0f6fc", fontWeight: 600 }}>"Cursor (204% CPU) — restart it."</span></div>
      <div style={{ marginTop: 24, fontSize: 40, color: "#3fb950", fontWeight: 700 }}>200 tokens. Done.</div>
    </div>
    <Footer right="7 / 10" />
  </Slide>
);

const S8 = () => (
  <Slide>
    <Tag color="#d2992a">The experiment</Tag>
    <div style={{ fontSize: 44, fontWeight: 700, color: "#f0f6fc", marginBottom: 56 }}>4 agents. One machine.</div>
    <div style={{ padding: "40px 40px", background: "rgba(248,81,73,0.04)", borderRadius: 20, border: "2px solid rgba(248,81,73,0.08)" }}>
      <div style={{ fontSize: 24, fontWeight: 700, color: "#f85149", textTransform: "uppercase", letterSpacing: "0.08em", marginBottom: 28 }}>Blind</div>
      <div style={{ display: "flex", gap: 80 }}>
        <div><div style={{ fontSize: 80, fontWeight: 800, color: "#f85149", lineHeight: 1 }}>99.97%</div><div style={{ fontSize: 24, color: "#8b949e", marginTop: 10 }}>CPU</div></div>
        <div><div style={{ fontSize: 80, fontWeight: 800, color: "#f85149", lineHeight: 1 }}>51.66%</div><div style={{ fontSize: 24, color: "#8b949e", marginTop: 10 }}>RAM</div></div>
      </div>
    </div>
    <div style={{ marginTop: 34, padding: "40px 40px", background: "rgba(63,185,80,0.04)", borderRadius: 20, border: "2px solid rgba(63,185,80,0.08)" }}>
      <div style={{ fontSize: 24, fontWeight: 700, color: "#3fb950", textTransform: "uppercase", letterSpacing: "0.08em", marginBottom: 28 }}>Axon-aware</div>
      <div style={{ display: "flex", gap: 80 }}>
        <div><div style={{ fontSize: 80, fontWeight: 800, color: "#3fb950", lineHeight: 1 }}>48.05%</div><div style={{ fontSize: 24, color: "#8b949e", marginTop: 10 }}>CPU</div></div>
        <div><div style={{ fontSize: 80, fontWeight: 800, color: "#3fb950", lineHeight: 1 }}>10.73%</div><div style={{ fontSize: 24, color: "#8b949e", marginTop: 10 }}>RAM</div></div>
      </div>
    </div>
    <div style={{ marginTop: 48, fontSize: 32, color: "#8b949e", textAlign: "center" }}>No scheduler. Just a shared view of reality.</div>
    <Footer right="8 / 10" />
  </Slide>
);

const S9 = () => (
  <Slide>
    <div style={{ textAlign: "center" }}>
      <Tag color="#3fb950">Setup</Tag>
      <div style={{ fontSize: 68, fontWeight: 800, color: "#f0f6fc", marginBottom: 70 }}>Two commands.</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
        <div style={{ fontFamily: "monospace", fontSize: 32, color: "#3fb950", background: "rgba(63,185,80,0.06)", padding: "34px 40px", borderRadius: 20, border: "2px solid rgba(63,185,80,0.1)", textAlign: "left" }}><span style={{ color: "#484f58" }}>$ </span>brew install rudraptpsingh/tap/axon</div>
        <div style={{ fontFamily: "monospace", fontSize: 32, color: "#3fb950", background: "rgba(63,185,80,0.06)", padding: "34px 40px", borderRadius: 20, border: "2px solid rgba(63,185,80,0.1)", textAlign: "left" }}><span style={{ color: "#484f58" }}>$ </span>axon setup</div>
      </div>
      <div style={{ marginTop: 60, fontSize: 34, color: "#c9d1d9" }}>Configures Claude, Cursor, VS Code.</div>
      <div style={{ marginTop: 28, fontSize: 42, color: "#f0f6fc", fontWeight: 700 }}>Agent restarts. Now it can see.</div>
    </div>
    <Footer right="9 / 10" />
  </Slide>
);

const S10 = () => (
  <Slide>
    <div style={{ textAlign: "center" }}>
      <div style={{ fontSize: 68, fontWeight: 800, color: "#f0f6fc" }}>Give your agent eyes.</div>
      <div style={{ marginTop: 70, fontFamily: "monospace", fontSize: 34, color: "#58a6ff", background: "rgba(88,166,255,0.06)", padding: "34px 50px", borderRadius: 20, border: "2px solid rgba(88,166,255,0.12)" }}>github.com/rudraptpsingh/axon</div>
      <div style={{ marginTop: 80, fontSize: 34, color: "#c9d1d9" }}>Ever lost work to an OOM crash?</div>
      <div style={{ marginTop: 18, fontSize: 34, color: "#f0f6fc", fontWeight: 600 }}>Drop a comment — I'll help you set up axon.</div>
    </div>
    <Footer right="10 / 10" />
  </Slide>
);

const allSlides = [S1, S2, S3, S4, S5, S6, S7, S8, S9, S10];

export default function Carousel() {
  const [idx, setIdx] = useState(0);
  const Current = allSlides[idx];

  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 14 }}>
      <div style={{ fontSize: 11, color: "#8b949e", textAlign: "center", maxWidth: 380, lineHeight: 1.5, padding: "6px 12px", background: "rgba(88,166,255,0.06)", borderRadius: 6 }}>
        Each slide is 1080×1350 (scaled to fit). Screenshot each one for pixel-perfect LinkedIn export.
      </div>
      <div style={{ width: W * SC, height: H * SC, borderRadius: 10, overflow: "hidden", boxShadow: "0 4px 24px rgba(0,0,0,0.4)", border: "1px solid rgba(255,255,255,0.06)" }}>
        <Current />
      </div>
      <div style={{ display: "flex", justifyContent: "center", gap: 4, marginTop: 4 }}>
        {allSlides.map((_, i) => (
          <div key={i} onClick={() => setIdx(i)} style={{ width: i === idx ? 16 : 5, height: 5, borderRadius: 3, background: i === idx ? "#58a6ff" : "rgba(150,150,150,0.3)", cursor: "pointer", transition: "all 0.2s" }} />
        ))}
      </div>
      <div style={{ display: "flex", gap: 12, fontFamily: "sans-serif" }}>
        <button onClick={() => setIdx(Math.max(0, idx - 1))} disabled={idx === 0}
          style={{ padding: "6px 16px", borderRadius: 6, border: "1px solid rgba(88,166,255,0.25)", background: idx === 0 ? "transparent" : "rgba(88,166,255,0.08)", color: idx === 0 ? "#555" : "#58a6ff", cursor: idx === 0 ? "default" : "pointer", fontSize: 13, fontWeight: 600 }}>← Prev</button>
        <span style={{ fontSize: 12, color: "#888", fontWeight: 600, display: "flex", alignItems: "center", minWidth: 50, justifyContent: "center" }}>{idx + 1} / {allSlides.length}</span>
        <button onClick={() => setIdx(Math.min(allSlides.length - 1, idx + 1))} disabled={idx === allSlides.length - 1}
          style={{ padding: "6px 16px", borderRadius: 6, border: "1px solid rgba(88,166,255,0.25)", background: idx === allSlides.length - 1 ? "transparent" : "rgba(88,166,255,0.08)", color: idx === allSlides.length - 1 ? "#555" : "#58a6ff", cursor: idx === allSlides.length - 1 ? "default" : "pointer", fontSize: 13, fontWeight: 600 }}>Next →</button>
      </div>
    </div>
  );
}
