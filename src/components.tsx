import * as Checkbox from "@radix-ui/react-checkbox";
import * as Progress from "@radix-ui/react-progress";
import * as Dialog from "@radix-ui/react-dialog";
import {
  Check,
  X,
  Image,
  Film,
  FileText,
  UserRound,
  MessageSquare,
  UsersRound,
  NotebookPen,
  type LucideIcon,
} from "lucide-react";
import type { Kind } from "./types";
import type { ReactNode } from "react";

export const icons: Record<Kind, LucideIcon> = {
  photos: Image,
  videos: Film,
  documents: FileText,
  profile: UserRound,
  wall: NotebookPen,
  friends: UsersRound,
  messages: MessageSquare,
};
export function CheckBox({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean | "indeterminate";
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <Checkbox.Root
      className="checkbox"
      checked={checked}
      onCheckedChange={(v) => onChange(v === true)}
      aria-label={label}
      disabled={disabled}
    >
      <Checkbox.Indicator>
        {checked === "indeterminate" ? (
          <span>−</span>
        ) : (
          <Check size={13} strokeWidth={3} />
        )}
      </Checkbox.Indicator>
    </Checkbox.Root>
  );
}
export function Bar({
  value,
  indeterminate = false,
  label,
  className = "",
}: {
  value: number;
  indeterminate?: boolean;
  label: string;
  className?: string;
}) {
  return (
    <Progress.Root
      className={`progress-track ${indeterminate ? "indeterminate" : ""} ${className}`}
      value={indeterminate ? null : Math.min(100, Math.max(0, value))}
      aria-label={label}
    >
      <Progress.Indicator
        style={{ transform: `translateX(-${100 - value}%)` }}
      />
    </Progress.Root>
  );
}
export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  wide = false,
  className = "",
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  title: string;
  description: string;
  children: ReactNode;
  wide?: boolean;
  className?: string;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="modal-overlay" />
        <Dialog.Content className={`modal ${wide ? "wide" : ""} ${className}`}>
          <div className="modal-heading">
            <div>
              <Dialog.Title>{title}</Dialog.Title>
              <Dialog.Description>{description}</Dialog.Description>
            </div>
            <Dialog.Close className="icon-button" aria-label="Закрыть">
              <X size={20} />
            </Dialog.Close>
          </div>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
