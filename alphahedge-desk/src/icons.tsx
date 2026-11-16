import type { ComponentChildren } from "preact";

// A handful of inline icons instead of an icon font: nothing to download,
// nothing held in memory beyond the few paths in use.
function Svg({ children, size = 16 }: { children: ComponentChildren; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      {children}
    </svg>
  );
}

export const Key = () => (<Svg><circle cx="8" cy="15" r="4" /><path d="M11 12l9-9M16 7l3 3" /></Svg>);
export const Refresh = () => (<Svg><path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7" /></Svg>);
export const Stop = () => (<Svg><rect x="6" y="6" width="12" height="12" rx="2" /></Svg>);
export const Trash = () => (<Svg><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13" /></Svg>);
export const Plus = () => (<Svg><path d="M12 5v14M5 12h14" /></Svg>);
export const CheckCircle = () => (<Svg><circle cx="12" cy="12" r="9" /><path d="M8 12l3 3 5-6" /></Svg>);
export const Alert = () => (<Svg><circle cx="12" cy="12" r="9" /><path d="M12 7v6M12 16.5v.5" /></Svg>);
export const Flask = () => (<Svg><path d="M9 3h6M10 3v6l-5 9a2 2 0 0 0 2 3h10a2 2 0 0 0 2-3l-5-9V3" /></Svg>);
export const Dashed = () => (<Svg><circle cx="12" cy="12" r="9" stroke-dasharray="3 3" /></Svg>);

export const NavBolt = () => (<Svg size={18}><path d="M13 3L5 14h6l-1 7 8-11h-6z" /></Svg>);
export const NavList = () => (<Svg size={18}><path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01" /></Svg>);
export const NavChart = () => (<Svg size={18}><path d="M4 20V10M10 20V4M16 20v-7M22 20H2" /></Svg>);
export const NavUsers = () => (<Svg size={18}><circle cx="9" cy="8" r="3.5" /><path d="M2.5 20a6.5 6.5 0 0 1 13 0M16 4.5a3.5 3.5 0 0 1 0 7M18 14a6.5 6.5 0 0 1 3.5 6" /></Svg>);
export const NavShield = () => (<Svg size={18}><path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z" /></Svg>);
export const NavBell = () => (<Svg size={18}><path d="M6 16v-5a6 6 0 1 1 12 0v5l2 2H4zM10 21h4" /></Svg>);

export const Moon = () => (<Svg><path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" /></Svg>);
export const Sun = () => (<Svg><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M19 5l-1.5 1.5M6.5 17.5L5 19" /></Svg>);
export const Monitor = () => (<Svg><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></Svg>);

export const Logout = () => (<Svg><path d="M10 4H5v16h5M15 8l4 4-4 4M19 12H9" /></Svg>);
export const Search = () => (<Svg><circle cx="11" cy="11" r="6" /><path d="M20 20l-4-4" /></Svg>);
export const ChevUp = () => (<Svg size={12}><path d="M7 14l5-5 5 5" /></Svg>);
export const ChevDown = () => (<Svg size={12}><path d="M7 10l5 5 5-5" /></Svg>);
