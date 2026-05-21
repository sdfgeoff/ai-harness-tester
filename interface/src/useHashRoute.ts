import { useEffect, useState } from "react";
import { parseHashRoute, type Route } from "./routes";

function currentRoute(): Route {
  return parseHashRoute(window.location.hash);
}

export function useHashRoute(): Route {
  const [route, setRoute] = useState<Route>(() => currentRoute());

  useEffect(() => {
    function onHashChange() {
      setRoute(currentRoute());
    }

    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  return route;
}
