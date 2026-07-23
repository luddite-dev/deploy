import { Center, Text } from "@mantine/core";
import { useParams } from "react-router-dom";
import {
  DeployStack,
  DestroyStack,
  PauseUnpauseStack,
  PullStack,
  RestartStack,
  StartStopStack,
} from "@/resources/stack/executions";
import { useStack } from "@/resources/stack";
import { useContainerPortsMap, useRead, useSetTitle } from "@/lib/hooks";
import { Types } from "komodo_client";
import { containerStateIntention } from "@/lib/color";
import { ICONS } from "@/lib/icons";
import ResourceLink from "@/resources/link";
import DockerResourceLink from "@/components/docker/link";
import { ContainerPort } from "@/components/docker/container-ports";
import StackServiceTabs from "./tabs";
import ResourceSubPage from "@/resources/sub-page";

type IdServiceComponent = React.FC<{ id: string; service?: string }>;

const Executions: { [action: string]: IdServiceComponent } = {
  DeployStack,
  PullStack,
  RestartStack,
  PauseUnpauseStack,
  StartStopStack,
  DestroyStack,
};

export default function StackService() {
  const {
    type,
    id: stackId,
    service: serviceName,
  } = useParams() as {
    type: string;
    id: string;
    service: string;
  };
  if (type !== "stacks") {
    return (
      <Center h="50vh">
        <Text>This resource type does not have any services.</Text>
      </Center>
    );
  }
  return <StackServiceInner stackId={stackId} serviceName={serviceName} />;
}

function StackServiceInner({
  stackId,
  serviceName,
}: {
  stackId: string;
  serviceName: string;
}) {
  const stack = useStack(stackId);
  useSetTitle(`${stack?.name} | ${serviceName}`);
  const services = useRead("ListStackServices", { stack: stackId }).data;
  const service = services?.find((s) => s.service === serviceName);

  const container = service?.container;

  const portsMap = useContainerPortsMap(container?.ports ?? []);

  const state = container?.state ?? Types.ContainerStateStatusEnum.Empty;

  const intention = containerStateIntention(
    container?.state ?? Types.ContainerStateStatusEnum.Empty,
  );

  return (
    <ResourceSubPage
      entityTypeName="Stack Service"
      parentType="Stack"
      parentId={stackId}
      name={serviceName}
      icon={ICONS.Service}
      intent={intention}
      state={state}
      status={container?.status}
      info={
        <>
          {stack?.info.server_id && (
            <>
              <ResourceLink type="Server" id={stack.info.server_id} />
              {container?.name && (
                <DockerResourceLink
                  type="Container"
                  serverId={stack.info.server_id}
                  name={container.name}
                />
              )}
              {container?.image && (
                <DockerResourceLink
                  type="Image"
                  serverId={stack.info.server_id}
                  name={container.image}
                  id={container.image_id}
                />
              )}
              {container?.networks?.map((network) => (
                <DockerResourceLink
                  key={network}
                  type="Network"
                  serverId={stack.info.server_id}
                  name={network}
                />
              ))}
              {container?.volumes?.map((volume) => (
                <DockerResourceLink
                  key={volume}
                  type="Volume"
                  serverId={stack.info.server_id}
                  name={volume}
                />
              ))}
              {Object.keys(portsMap).map((hostPort) => (
                <ContainerPort
                  key={hostPort}
                  hostPort={hostPort}
                  ports={portsMap[hostPort]}
                  serverId={stack.info.server_id}
                />
              ))}
            </>
          )}
        </>
      }
      executions={
        <>
          {Object.entries(Executions).map(([key, Execution]) => (
            <Execution key={key} id={stackId} service={serviceName} />
          ))}
        </>
      }
    >
      {stack && (
        <StackServiceTabs
          stack={stack}
          service={serviceName}
          container={container}
          intention={intention}
        />
      )}
    </ResourceSubPage>
  );
}
