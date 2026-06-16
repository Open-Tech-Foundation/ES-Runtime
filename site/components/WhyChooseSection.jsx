export default function WhyChooseSection() {
  return (
    <section className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
      <div className="text-center mb-12">
        <h2 className="text-3xl font-bold tracking-tight text-zinc-900">
          Why ESRun?
        </h2>
        <p className="mt-4 text-lg text-zinc-600 max-w-2xl mx-auto">
          Compare how different runtimes behave when deployed to your server instance.
        </p>
      </div>

      <div className="rounded-2xl bg-zinc-950 p-8 lg:p-10 shadow-inner border border-zinc-800 relative overflow-hidden">
        {/* Subtle background glow */}
        <div className="absolute top-0 left-1/2 -translate-x-1/2 w-3/4 h-32 bg-brand-500/10 blur-[100px] pointer-events-none"></div>
        
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-8 relative z-10">
          
          {/* Node.js */}
          <div className="flex flex-col">
            <div className="flex items-center justify-center gap-2 mb-4">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="#22c55e" xmlns="http://www.w3.org/2000/svg">
                <path d="M11.998,24c-0.321,0-0.641-0.084-0.922-0.247l-2.936-1.737c-0.438-0.245-0.224-0.332-0.08-0.383 c0.585-0.203,0.703-0.25,1.328-0.604c0.065-0.037,0.151-0.023,0.218,0.017l2.256,1.339c0.082,0.045,0.197,0.045,0.272,0l8.795-5.076 c0.082-0.047,0.134-0.141,0.134-0.238V6.921c0-0.099-0.053-0.192-0.137-0.242l-8.791-5.072c-0.081-0.047-0.189-0.047-0.271,0 L3.075,6.68C2.99,6.729,2.936,6.825,2.936,6.921v10.15c0,0.097,0.054,0.189,0.139,0.235l2.409,1.392 c1.307,0.654,2.108-0.116,2.108-0.89V7.787c0-0.142,0.114-0.253,0.256-0.253h1.115c0.139,0,0.255,0.112,0.255,0.253v10.021 c0,1.745-0.95,2.745-2.604,2.745c-0.508,0-0.909,0-2.026-0.551L2.28,18.675c-0.57-0.329-0.922-0.945-0.922-1.604V6.921 c0-0.659,0.353-1.275,0.922-1.603l8.795-5.082c0.557-0.315,1.296-0.315,1.848,0l8.794,5.082c0.57,0.329,0.924,0.944,0.924,1.603 v10.15c0,0.659-0.354,1.273-0.924,1.604l-8.794,5.078C12.643,23.916,12.324,24,11.998,24z M19.099,13.993 c0-1.9-1.284-2.406-3.987-2.763c-2.731-0.361-3.009-0.548-3.009-1.187c0-0.528,0.235-1.233,2.258-1.233 c1.807,0,2.473,0.389,2.747,1.607c0.024,0.115,0.129,0.199,0.247,0.199h1.141c0.071,0,0.138-0.031,0.186-0.081 c0.048-0.054,0.074-0.123,0.067-0.196c-0.177-2.098-1.571-3.076-4.388-3.076c-2.508,0-4.004,1.058-4.004,2.833 c0,1.925,1.488,2.457,3.895,2.695c2.88,0.282,3.103,0.703,3.103,1.269c0,0.983-0.789,1.402-2.642,1.402 c-2.327,0-2.839-0.584-3.011-1.742c-0.02-0.124-0.126-0.215-0.253-0.215h-1.137c-0.141,0-0.254,0.112-0.254,0.253 c0,1.482,0.806,3.248,4.655,3.248C17.501,17.007,19.099,15.91,19.099,13.993z"/>
              </svg>
              <h3 className="font-bold text-zinc-100 tracking-wide">Node.js</h3>
            </div>
            <div className="flex-1 bg-zinc-900/80 border-t-4 border-zinc-800 border-t-green-500 rounded-xl p-5 shadow-lg relative group transition-colors hover:bg-zinc-900 hover:border-zinc-700">
              <div className="absolute top-4 right-4 opacity-10 group-hover:opacity-30 transition-opacity">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="text-zinc-300">
                  <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
                  <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
                  <line x1="6" y1="6" x2="6.01" y2="6"></line>
                  <line x1="6" y1="18" x2="6.01" y2="18"></line>
                </svg>
              </div>
              <ul className="mt-2 text-[13px] text-zinc-400 space-y-3 relative z-10 leading-relaxed">
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Largest package ecosystem</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Deep native addon support</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Battle-tested stability</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Elevated idle memory footprint</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Legacy API (CommonJS) baggage</li>
              </ul>
            </div>
          </div>

          {/* Deno */}
          <div className="flex flex-col">
            <div className="flex items-center justify-center gap-2 mb-4">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="#f4f4f5" xmlns="http://www.w3.org/2000/svg">
                <path d="M1.105 18.02A11.9 11.9 0 0 1 0 12.985q0-.698.078-1.376a12 12 0 0 1 .231-1.34A12 12 0 0 1 4.025 4.02a12 12 0 0 1 5.46-2.771 12 12 0 0 1 3.428-.23c1.452.112 2.825.477 4.077 1.05a12 12 0 0 1 2.78 1.774 12.02 12.02 0 0 1 4.053 7.078A12 12 0 0 1 24 12.985q0 .454-.036.914a12 12 0 0 1-.728 3.305 12 12 0 0 1-2.38 3.875c-1.33 1.357-3.02 1.962-4.43 1.936a4.4 4.4 0 0 1-2.724-1.024c-.99-.853-1.391-1.83-1.53-2.919a5 5 0 0 1 .128-1.518c.105-.38.37-1.116.76-1.437-.455-.197-1.04-.624-1.226-.829-.045-.05-.04-.13 0-.183a.155.155 0 0 1 .177-.053c.392.134.869.267 1.372.35.66.111 1.484.25 2.317.292 2.03.1 4.153-.813 4.812-2.627s.403-3.609-1.96-4.685-3.454-2.356-5.363-3.128c-1.247-.505-2.636-.205-4.06.582-3.838 2.121-7.277 8.822-5.69 15.032a.191.191 0 0 1-.315.19 12 12 0 0 1-1.25-1.634 12 12 0 0 1-.769-1.404M11.57 6.087c.649-.051 1.214.501 1.31 1.236.13.979-.228 1.99-1.41 2.013-1.01.02-1.315-.997-1.248-1.614.066-.616.574-1.575 1.35-1.635"/>
              </svg>
              <h3 className="font-bold text-zinc-100 tracking-wide">Deno</h3>
            </div>
            <div className="flex-1 bg-zinc-900/80 border-t-4 border-zinc-800 border-t-zinc-400 rounded-xl p-5 shadow-lg relative group transition-colors hover:bg-zinc-900 hover:border-zinc-700">
              <div className="absolute top-4 right-4 opacity-10 group-hover:opacity-30 transition-opacity">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="text-zinc-300">
                  <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
                  <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
                  <line x1="6" y1="6" x2="6.01" y2="6"></line>
                  <line x1="6" y1="18" x2="6.01" y2="18"></line>
                </svg>
              </div>
              <ul className="mt-2 text-[13px] text-zinc-400 space-y-3 relative z-10 leading-relaxed">
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Secure by default</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Built-in TypeScript support</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Native standard library</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Custom namespace fragmentation</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Heavy isolate initialization</li>
              </ul>
            </div>
          </div>

          {/* Bun */}
          <div className="flex flex-col">
            <div className="flex items-center justify-center gap-2 mb-4">
              <img src="/bun.svg" alt="Bun" width="24" height="24" />
              <h3 className="font-bold text-zinc-100 tracking-wide">Bun</h3>
            </div>
            <div className="flex-1 bg-zinc-900/80 border-t-4 border-zinc-800 border-t-pink-400 rounded-xl p-5 shadow-lg relative group transition-colors hover:bg-zinc-900 hover:border-zinc-700">
              <div className="absolute top-4 right-4 opacity-10 group-hover:opacity-30 transition-opacity">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="text-zinc-300">
                  <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
                  <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
                  <line x1="6" y1="6" x2="6.01" y2="6"></line>
                  <line x1="6" y1="18" x2="6.01" y2="18"></line>
                </svg>
              </div>
              <ul className="mt-2 text-[13px] text-zinc-400 space-y-3 relative z-10 leading-relaxed">
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Exceptional JIT performance</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Integrated bundler & test runner</li>
                <li className="flex items-start gap-2"><span className="text-green-500 font-bold shrink-0">✓</span> Node.js API compatibility</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Aggressive memory peaks</li>
                <li className="flex items-start gap-2"><span className="text-red-400 font-bold shrink-0">✗</span> Memory safety regressions</li>
              </ul>
            </div>
          </div>

          {/* ESRun */}
          <div className="flex flex-col">
            <div className="flex items-center justify-center gap-2 mb-4">
              <span className="flex h-6 w-6 items-center justify-center rounded-full bg-brand-500 text-white shadow-md shrink-0">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"><polyline points="20 6 9 17 4 12"></polyline></svg>
              </span>
              <h3 className="font-bold text-zinc-100 tracking-wide">ESRun</h3>
            </div>
            <div className="flex-1 bg-zinc-900 border-t-4 border-brand-500 rounded-xl p-5 shadow-lg relative group transition-colors hover:bg-zinc-800">
              <div className="absolute top-4 right-4 opacity-20 group-hover:opacity-40 transition-opacity">
                <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="text-brand-400">
                  <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
                  <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
                  <line x1="6" y1="6" x2="6.01" y2="6"></line>
                  <line x1="6" y1="18" x2="6.01" y2="18"></line>
                </svg>
              </div>
              <ul className="mt-2 text-[13px] text-zinc-300 space-y-3 relative z-10 leading-relaxed">
                <li className="flex items-start gap-2">
                  <span className="text-brand-500 font-bold shrink-0">✓</span> 
                  <span><strong className="text-zinc-100">WinterTC Standard</strong> – Pure Web APIs without custom namespace lock-in.</span>
                </li>
                <li className="flex items-start gap-2">
                  <span className="text-brand-500 font-bold shrink-0">✓</span> 
                  <span><strong className="text-zinc-100">Predictable Safety</strong> – Strict memory bounds without native segmentation faults.</span>
                </li>
                <li className="flex items-start gap-2">
                  <span className="text-brand-500 font-bold shrink-0">✓</span> 
                  <span><strong className="text-zinc-100">Lightweight Footprint</strong> – Operates with a &lt;20MB baseline.</span>
                </li>
                <li className="flex items-start gap-2">
                  <span className="text-brand-500 font-bold shrink-0">✓</span> 
                  <span><strong className="text-zinc-100">Fast Initialization</strong> – Boots in ~7ms without V8 snapshot bloat.</span>
                </li>
              </ul>
            </div>
          </div>

        </div>
      </div>
    </section>
  );
}
