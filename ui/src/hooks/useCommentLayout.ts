import { useEffect, useState } from "react";

/** `dense`: the comment list beside the page; `margin`: classic Word, each comment next to the text it marks. */
export type CommentLayout = "dense" | "margin";

const KEY = "commentLayout";

function read(): CommentLayout {
  try {
    return localStorage.getItem(KEY) === "margin" ? "margin" : "dense";
  } catch {
    return "dense";
  }
}

export function useCommentLayout(): [CommentLayout, (layout: CommentLayout) => void] {
  const [layout, setLayout] = useState<CommentLayout>(read);
  useEffect(() => {
    try {
      localStorage.setItem(KEY, layout);
    } catch {
      /* storage unavailable */
    }
  }, [layout]);
  return [layout, setLayout];
}
