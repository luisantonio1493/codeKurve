export class MemberService {
  find(id: string): string {
    return id;
  }
}

export function createMemberService(): MemberService {
  return new MemberService();
}
