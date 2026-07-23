import { useState } from "react";
import NewResource from "./new";
import { Types } from "komodo_client";
import ResourceSelector from "./selector";

/** Used by Stacks and Deployments */
export default function NewResourceWithDeployTarget({
  type,
  serverId: _serverId,
}: {
  type: "Stack" | "Deployment";
  serverId?: string;
}) {
  const [serverId, setServerId] = useState("");
  return (
    <NewResource<Types.DeploymentConfig>
      type={type}
      config={() => ({
        server_id: _serverId ?? serverId,
      })}
      extraInputs={
        !_serverId ? (
          <>
            <ResourceSelector
              type="Server"
              selected={serverId}
              onSelect={setServerId}
              targetProps={{ w: "100%", maw: "100%" }}
              width="target"
              position="bottom"
              clearable
            />
          </>
        ) : undefined
      }
      showTemplateSelector={!!_serverId || !serverId}
    />
  );
}
