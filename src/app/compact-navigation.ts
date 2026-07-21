import { createContext, useContext } from "react";

export const CompactBackToHomeContext = createContext<(() => void) | null>(null);

export function useCompactBackToHome() {
  return useContext(CompactBackToHomeContext);
}

export enum CompactRegion {
  Home = "home",
  Editor = "editor",
  Assistant = "assistant",
}

export type CompactNavigationState = Readonly<{
  region: CompactRegion;
  returnRegion: Exclude<CompactRegion, CompactRegion.Assistant>;
}>;

export type CompactNavigationAction =
  | { readonly type: "open_home" }
  | { readonly type: "open_editor" }
  | { readonly type: "open_assistant" }
  | { readonly type: "close_assistant" };

export const initialCompactNavigation: CompactNavigationState = {
  region: CompactRegion.Home,
  returnRegion: CompactRegion.Home,
};

export function compactNavigationReducer(
  state: CompactNavigationState,
  action: CompactNavigationAction,
): CompactNavigationState {
  switch (action.type) {
    case "open_home":
      return { region: CompactRegion.Home, returnRegion: CompactRegion.Home };
    case "open_editor":
      return { region: CompactRegion.Editor, returnRegion: CompactRegion.Editor };
    case "open_assistant":
      return {
        region: CompactRegion.Assistant,
        returnRegion: state.region === CompactRegion.Assistant ? state.returnRegion : state.region,
      };
    case "close_assistant":
      return { region: state.returnRegion, returnRegion: state.returnRegion };
  }
}

export function workbenchIsVisible(region: CompactRegion) {
  return region === CompactRegion.Home || region === CompactRegion.Editor;
}
