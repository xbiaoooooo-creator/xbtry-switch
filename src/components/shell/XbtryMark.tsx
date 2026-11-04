import { cn } from "@/lib/utils";
import logo from "@/assets/icons/logo.png";
export function XbtryMark({ className }: { className?: string }) {
  return (
    <img
      src={logo}
      alt=""
      aria-hidden="true"
      draggable={false}
      className={cn("h-9 w-9 shrink-0 rounded-lg", className)}
    />
  );
}
