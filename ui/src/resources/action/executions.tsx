import ConfirmModalWithDisable from "@/components/confirm-modal-with-disable";
import { useExecute, useInvalidate, useRead } from "@/lib/hooks";
import { ICONS } from "@/lib/icons";
import { EXECUTION_ACTION_STATE_REQUERY_MS } from "@/lib/utils";
import { useAction } from ".";

export function RunAction({ id }: { id: string }) {
  const invalidate = useInvalidate();
  const running = useRead(
    "GetActionActionState",
    { action: id },
    { refetchInterval: 5_000 },
  ).data?.running;
  const { mutateAsync: run, isPending } = useExecute("RunAction", {
    onSuccess: () =>
      setTimeout(
        () => invalidate(["GetActionActionState"]),
        EXECUTION_ACTION_STATE_REQUERY_MS,
      ),
  });
  const action = useAction(id);
  if (!action) return null;
  return (
    <ConfirmModalWithDisable
      confirmText={action.name}
      icon={<ICONS.Run size="1rem" />}
      onConfirm={() => run({ action: id })}
      disabled={!!running || isPending}
      loading={!!running || isPending}
    >
      {running ? "Running" : "Run Action"}
    </ConfirmModalWithDisable>
  );
}
